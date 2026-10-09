import { FormField } from "../FormField";
import { basisClass, basisLabel, FIELD_ROWS, type FieldForm, type FieldKey } from "./shared";

/** The profile fields, each with its provenance and the per-field confirmation checkbox. */
export function ProfileFieldsGrid({
  fields,
  onFieldChange,
}: {
  fields: Record<FieldKey, FieldForm>;
  onFieldChange: (key: FieldKey, patch: Partial<FieldForm>) => void;
}) {
  return (
    <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
      {FIELD_ROWS.map((row) => (
        <FormField key={row.key} label={row.label} hint={row.hint}>
          {row.multiline === true ? (
            <textarea
              className="textarea"
              rows={3}
              value={fields[row.key].value}
              onChange={(event) => {
                onFieldChange(row.key, { value: event.target.value });
              }}
            />
          ) : (
            <input
              className="input"
              value={fields[row.key].value}
              onChange={(event) => {
                onFieldChange(row.key, { value: event.target.value });
              }}
            />
          )}
          <div className="mt-1 flex items-center justify-between gap-2">
            <label className="flex items-center gap-1 text-xs text-muted">
              <input
                type="checkbox"
                checked={fields[row.key].confirmed}
                onChange={(event) => {
                  onFieldChange(row.key, { confirmed: event.target.checked });
                }}
              />
              Conferma
            </label>
            <span className={basisClass(fields[row.key].basis)}>
              {basisLabel(fields[row.key].basis)}
            </span>
          </div>
        </FormField>
      ))}
    </div>
  );
}
