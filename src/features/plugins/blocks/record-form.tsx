import { useCallback, useId, useState } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { usePollingResource } from "@/shared/hooks/use-polling-resource";
import { useWorkspaceNavigation } from "@/shared/hooks/use-workspace-navigation";
import { projectsApi } from "@/features/projects/api";
import { sessionsApi } from "@/features/sessions/api";
import type {
  CollectionDef,
  FieldDef,
} from "../../../../contracts/plugins/platform/types";

export type RecordValues = Record<string, unknown>;

function ReferencePicker({
  id,
  value,
  kind,
  onChange,
}: {
  id: string;
  value: string | null;
  kind: "ref:session" | "ref:project";
  onChange: (next: string | null) => void;
}) {
  const [text, setText] = useState("");
  const [open, setOpen] = useState(false);
  const load = useCallback(async () => {
    if (kind === "ref:session") {
      const response = await sessionsApi.sessions({ query: text.trim() });
      return response.items.map((item) => [item.id, item.title] as const);
    }
    const response = await projectsApi.projects({ query: text.trim() });
    return response.items.map((item) => [item.id, item.name] as const);
  }, [kind, text]);
  const result = usePollingResource(`${kind}:${text}`, load, open, 60_000);
  const labels = new Map(result.data ?? []);
  return (
    <Combobox<string>
      open={open}
      onOpenChange={setOpen}
      value={value}
      items={[...labels.keys()]}
      filter={null}
      onInputValueChange={(next) => setText(next)}
      itemToStringLabel={(key) => labels.get(key) ?? key}
      onValueChange={onChange}
    >
      <ComboboxInput id={id} placeholder="搜索本机记录" />
      <ComboboxContent>
        <ComboboxEmpty>
          {result.error ?? (result.loading ? "正在读取…" : "无匹配记录")}
        </ComboboxEmpty>
        <ComboboxList>
          {(key: string) => (
            <ComboboxItem key={key} value={key}>
              {labels.get(key)}
            </ComboboxItem>
          )}
        </ComboboxList>
      </ComboboxContent>
    </Combobox>
  );
}

function FieldInput({
  id,
  field,
  value,
  onChange,
  locked,
}: {
  id: string;
  field: FieldDef;
  value: unknown;
  onChange: (value: unknown) => void;
  locked: boolean;
}) {
  const text = typeof value === "string" ? value : "";
  if (locked) return <Input id={id} readOnly value={text} />;
  if (field.kind === "boolean")
    return (
      <Checkbox
        id={id}
        checked={value === true}
        onCheckedChange={(checked) => onChange(checked === true)}
      />
    );
  if (field.kind === "enum")
    return (
      <Select
        value={text || undefined}
        onValueChange={(next) => onChange(next ?? null)}
      >
        <SelectTrigger id={id} placeholder={field.label} />
        <SelectContent>
          {(field.options ?? []).map((option) => (
            <SelectItem key={option} value={option}>
              {option}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    );
  if (field.kind === "ref:session" || field.kind === "ref:project")
    return (
      <ReferencePicker
        id={id}
        kind={field.kind}
        value={text || null}
        onChange={onChange}
      />
    );
  if (field.kind === "textarea")
    return (
      <Textarea
        id={id}
        value={text}
        required={field.required}
        onChange={(event) => onChange(event.target.value)}
      />
    );
  if (field.kind === "number")
    return (
      <Input
        id={id}
        type="number"
        step="any"
        required={field.required}
        value={typeof value === "number" ? value : ""}
        onChange={(event) =>
          onChange(
            Number.isFinite(event.target.valueAsNumber)
              ? event.target.valueAsNumber
              : null,
          )
        }
      />
    );
  return (
    <Input
      id={id}
      value={text}
      required={field.required}
      onChange={(event) => onChange(event.target.value || null)}
    />
  );
}

export function RecordForm({
  collection,
  initial,
  lockedField,
  primaryField,
  title,
  error,
  onClose,
  onSave,
}: {
  collection: CollectionDef;
  initial: RecordValues;
  lockedField?: string | null;
  primaryField: string;
  title: string;
  error?: string;
  onClose: () => void;
  onSave: (values: RecordValues) => Promise<boolean>;
}) {
  const workspace = useWorkspaceNavigation();
  const [values, setValues] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [saveAttempted, setSaveAttempted] = useState(false);
  const baseId = useId();
  const fields = Object.entries(collection.fields);
  fields.sort(
    ([a], [b]) => Number(b === primaryField) - Number(a === primaryField),
  );
  const close = () => {
    workspace?.setDirty(false);
    onClose();
  };
  const valid = fields.every(
    ([key, field]) =>
      !field.required ||
      (Object.hasOwn(values, key) && values[key] != null && values[key] !== ""),
  );
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) close();
      }}
    >
      <DialogContent className="max-h-[85dvh] overflow-y-auto">
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>填写要保存到此插件集合的字段。</DialogDescription>
        <form
          className="space-y-4"
          onSubmit={async (event) => {
            event.preventDefault();
            if (busy || !valid) return;
            setSaveAttempted(true);
            setBusy(true);
            try {
              if (await onSave(values)) close();
            } finally {
              setBusy(false);
            }
          }}
        >
          {fields.map(([key, field]) => (
            <Field key={key}>
              <FieldLabel htmlFor={`${baseId}-${key}`}>
                {field.label}
                {field.required ? " *" : ""}
              </FieldLabel>
              <FieldInput
                id={`${baseId}-${key}`}
                field={field}
                value={Object.hasOwn(values, key) ? values[key] : null}
                locked={lockedField === key}
                onChange={(next) => {
                  setValues((previous) => ({ ...previous, [key]: next }));
                  workspace?.setDirty(true);
                }}
              />
            </Field>
          ))}
          {saveAttempted && error && (
            <p role="alert" className="text-label text-fg-muted">
              {error}
            </p>
          )}
          <DialogFooter>
            <Button
              type="button"
              variant="secondary"
              disabled={busy}
              onClick={close}
            >
              取消
            </Button>
            <Button type="submit" loading={busy} disabled={!valid}>
              保存
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
