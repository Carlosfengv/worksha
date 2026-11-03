use super::*;

/// A checkpoint of this worker only. It does not fence foreign SQLite writers,
/// rotate content epochs, authorize deletion, or perform a cleanup handoff.
pub struct Checkpoint<'a> {
    writer: &'a Writer,
    id: u64,
    epoch: ContentEpoch,
}

pub(super) fn release(state: &mut State, id: Option<u64>) {
    if id.is_some_and(|id| state.checkpoint.as_ref().is_none_or(|pause| pause.id != id)) {
        return;
    }
    let Some(pause) = state.checkpoint.take() else {
        return;
    };
    for (id, row) in pause.rows {
        if state
            .rows
            .get(&id)
            .is_none_or(|old| old.revision < row.revision)
        {
            state.rows.insert(id, row);
        }
    }
    for (id, input) in pause.inputs {
        if state
            .inputs
            .get(&id)
            .is_none_or(|old| old.input.at <= input.input.at)
        {
            state.inputs.insert(id, input);
        }
    }
}

fn invalid(state: &State) -> Option<String> {
    if state.scope_changed {
        Some(ObservationWriteError::ScopeChanged.to_string())
    } else if state.stopped {
        Some("观测写入器正在退出".into())
    } else if state.health.dropped_updates > 0 {
        Some("部分观测更新未入队，完整性未确认".into())
    } else {
        None
    }
}

impl Writer {
    /// Drain the admitted frontier and await the worker's own epoch check.
    /// Updates arriving meanwhile stay bounded and are resumed by guard drop.
    pub fn checkpoint(&self, timeout: Duration) -> Result<super::Checkpoint<'_>, String> {
        let deadline = Instant::now() + timeout;
        let (lock, changed) = &*self.shared;
        let mut state = lock.lock().map_err(|_| "观测写入器不可用")?;
        if let Some(error) = invalid(&state) {
            return Err(error);
        }
        if state.checkpoint.is_some() {
            return Err("观测写入检查正在进行，请稍后重试".into());
        }
        if state.epoch.is_none() && state.pending() == 0 {
            return Err("观测写入来源尚未核查，不能确认已排空".into());
        }
        let id = state.next_checkpoint;
        state.next_checkpoint = id.checked_add(1).ok_or("观测写入检查编号已耗尽")?;
        state.checkpoint = Some(CheckpointState {
            id,
            ready: false,
            rows: HashMap::new(),
            inputs: HashMap::new(),
        });
        changed.notify_all();
        loop {
            if let Some(error) = invalid(&state) {
                release(&mut state, Some(id));
                changed.notify_all();
                return Err(error);
            }
            let pause = state
                .checkpoint
                .as_ref()
                .filter(|pause| pause.id == id)
                .ok_or("观测写入检查已取消")?;
            if pause.ready {
                if let Some(epoch) = state.epoch.clone() {
                    return Ok(Checkpoint {
                        writer: self,
                        id,
                        epoch,
                    });
                }
                release(&mut state, Some(id));
                changed.notify_all();
                return Err("观测内容版本不可确认".into());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                release(&mut state, Some(id));
                changed.notify_all();
                return Err("观测写入尚未全部提交，请检查存储状态".into());
            }
            state = match changed.wait_timeout(state, remaining) {
                Ok((next, _)) => next,
                Err(poisoned) => {
                    let (mut state, _) = poisoned.into_inner();
                    release(&mut state, Some(id));
                    changed.notify_all();
                    return Err("观测写入器不可用".into());
                }
            };
        }
    }
}

impl Checkpoint<'_> {
    pub fn epoch(&self) -> &ContentEpoch {
        &self.epoch
    }

    pub fn is_empty(&self) -> Result<bool, String> {
        let state = self
            .writer
            .shared
            .0
            .lock()
            .map_err(|_| "观测写入器不可用")?;
        if let Some(error) = invalid(&state) {
            return Err(error);
        }
        let pause = state
            .checkpoint
            .as_ref()
            .filter(|pause| pause.id == self.id && pause.ready)
            .ok_or("观测写入检查已失效")?;
        if state.epoch.as_ref() != Some(self.epoch()) {
            return Err(ObservationWriteError::ScopeChanged.to_string());
        }
        Ok(pause.rows.is_empty()
            && pause.inputs.is_empty()
            && state.rows.is_empty()
            && state.inputs.is_empty()
            && state.writing == 0
            && state.checking_checkpoint.is_none())
    }
}

impl Drop for Checkpoint<'_> {
    fn drop(&mut self) {
        let (lock, changed) = &*self.writer.shared;
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        release(&mut state, Some(self.id));
        changed.notify_all();
    }
}
