use std::collections::{BTreeMap, BTreeSet};

use local_agent_core::plugin::{
    external::{Manifest, Runtime},
    platform::*,
};

use super::{Checked, Result, fail, package};

pub(super) fn opted_in(manifest: &Manifest) -> bool {
    manifest
        .capabilities
        .iter()
        .any(|capability| capability == CAPABILITY)
}

pub(super) fn parse(
    manifest: &Manifest,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<Option<PlatformDefinition>> {
    if !opted_in(manifest) {
        if manifest.capabilities.iter().any(|capability| {
            [STORAGE, DIRECTORY_SOURCE, PROVIDER_ADAPTER].contains(&capability.as_str())
        }) {
            return Err(fail("permission_denied"));
        }
        return Ok(None);
    }
    let bytes = files.get("platform.json").ok_or(fail("invalid_input"))?;
    if bytes.len() > MAX_DECLARATION_BYTES {
        return Err(fail("resource_limit"));
    }
    let definition: PlatformDefinition = serde_json::from_slice(bytes).checked("invalid_input")?;
    validate(manifest, &definition)?;
    Ok(Some(definition))
}

fn title(value: &str) -> bool {
    !value.is_empty() && value.len() <= 120 && !value.chars().any(char::is_control)
}

fn validate(manifest: &Manifest, definition: &PlatformDefinition) -> Result<()> {
    if definition.schema_version != 1 {
        return Err(fail("incompatible_api"));
    }
    let directory = manifest
        .capabilities
        .iter()
        .any(|capability| capability == DIRECTORY_SOURCE);
    let provider = manifest
        .capabilities
        .iter()
        .any(|capability| capability == PROVIDER_ADAPTER);
    if directory != matches!(definition.adapter, Some(AdapterDef::AgentAdapter { .. }))
        || provider != matches!(definition.adapter, Some(AdapterDef::ProviderAdapter))
    {
        return Err(fail("permission_denied"));
    }
    if let Some(AdapterDef::AgentAdapter { source_mapper }) = &definition.adapter {
        super::platform::validate_mapper(source_mapper)?;
    }
    if definition.collections.len() > 16
        || definition.views.len() > 24
        || definition.slots.len() > 32
    {
        return Err(fail("resource_limit"));
    }
    if !definition.collections.is_empty()
        && !manifest
            .capabilities
            .iter()
            .any(|capability| capability == STORAGE)
    {
        return Err(fail("permission_denied"));
    }
    for (id, collection) in &definition.collections {
        if !package::local_id(id) || collection.fields.is_empty() || collection.fields.len() > 32 {
            return Err(fail("invalid_input"));
        }
        for (id, field) in &collection.fields {
            if !package::local_id(id)
                || !title(&field.label)
                || (field.kind == FieldKind::Enum) == field.options.is_empty()
                || field.options.len() > 32
                || field.options.iter().any(|option| !title(option))
                || field.options.iter().collect::<BTreeSet<_>>().len() != field.options.len()
            {
                return Err(fail("invalid_input"));
            }
        }
        if collection.group_by.as_ref().is_some_and(|key| {
            !collection
                .fields
                .get(key)
                .is_some_and(|field| field.kind == FieldKind::Enum && field.required)
        }) {
            return Err(fail("invalid_input"));
        }
    }
    for (id, view) in &definition.views {
        let collection = view
            .collection
            .as_ref()
            .and_then(|key| definition.collections.get(key));
        if !package::local_id(id)
            || !title(&view.title)
            || (view.collection.is_some() && collection.is_none())
            || (view.kind == ViewKind::Kanban
                && collection.is_none_or(|item| item.group_by.is_none()))
            || view
                .primary_field
                .as_ref()
                .is_some_and(|key| collection.is_none_or(|item| !item.fields.contains_key(key)))
            || view.actions.len() > 5
            || view.actions.iter().any(|action| match action {
                PlatformAction::SetValue => !manifest
                    .capabilities
                    .iter()
                    .any(|capability| capability == STORAGE),
                _ => collection.is_none(),
            })
        {
            return Err(fail("invalid_input"));
        }
    }
    let mut ids = BTreeSet::new();
    for slot in &definition.slots {
        let view = definition
            .views
            .get(&slot.view_id)
            .ok_or(fail("invalid_input"))?;
        let reference_target = slot.filter_by_context.as_ref().map(|field| {
            view.collection
                .as_ref()
                .and_then(|key| definition.collections.get(key))
                .and_then(|collection| collection.fields.get(field))
                .and_then(|field| field.kind.reference())
        });
        if !package::local_id(&slot.id)
            || definition.views.contains_key(&slot.id)
            || !ids.insert(&slot.id)
            || (slot.point != SlotPoint::NavHomeSidebar
                && (slot.group.is_some() || slot.icon.is_some()))
            || slot
                .icon
                .as_ref()
                .is_some_and(|icon| !package::local_id(icon))
            || reference_target
                .is_some_and(|target| target.is_none() || target != slot.point.target())
            || (is_actions(slot.point) && view.kind != ViewKind::ActionBar)
        {
            return Err(fail("invalid_input"));
        }
    }
    if definition
        .log_sink
        .as_ref()
        .is_some_and(|sink| matches!(sink.format, LogFormat::Wasm))
        && !matches!(manifest.runtime, Runtime::WasmComponent { .. })
    {
        return Err(fail("invalid_input"));
    }
    if definition.wasm.is_some() && !matches!(manifest.runtime, Runtime::WasmComponent { .. }) {
        return Err(fail("invalid_input"));
    }
    Ok(())
}

fn is_actions(point: SlotPoint) -> bool {
    matches!(
        point,
        SlotPoint::SessionsToolbarActions
            | SlotPoint::RequestsToolbarActions
            | SlotPoint::SessionDetailActions
            | SlotPoint::ProjectDetailActions
            | SlotPoint::RequestDetailActions
            | SlotPoint::AgentDetailActions
            | SlotPoint::ProviderDetailActions
            | SlotPoint::AccountDetailActions
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn platform_schemas_match_committed_artifacts() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/plugins/platform");
        for (name, schema) in local_agent_core::plugin::platform::schemas()
            .as_object()
            .unwrap()
        {
            let committed: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(&committed, schema);
        }
    }
}
