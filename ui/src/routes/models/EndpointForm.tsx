import { useState } from "react";
import { FormField } from "../../components/FormField";
import { endpointUpsert, toErrorMessage } from "../../lib/ipc";
import type { Endpoint } from "../../lib/types";
import {
  EMPTY_ENDPOINT_FORM,
  type EndpointFormErrors,
  type EndpointFormState,
} from "./shared";

function formFrom(endpoint: Endpoint | null): EndpointFormState {
  if (endpoint === null) {
    return EMPTY_ENDPOINT_FORM;
  }
  return {
    id: endpoint.id,
    name: endpoint.name,
    base_url: endpoint.base_url,
    api_key_ref: endpoint.api_key_ref ?? "",
    max_concurrency: endpoint.max_concurrency === null ? "" : String(endpoint.max_concurrency),
    notes: endpoint.notes ?? "",
  };
}

/** Create or edit one endpoint; the parent mounts it with the endpoint to edit, or `null`. */
export function EndpointForm({
  initial,
  onSaved,
  onClose,
}: {
  initial: Endpoint | null;
  onSaved: (saved: Endpoint) => void;
  onClose: () => void;
}) {
  const [form, setForm] = useState<EndpointFormState>(() => formFrom(initial));
  const [formErrors, setFormErrors] = useState<EndpointFormErrors>({});
  const [formError, setFormError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  function validateEndpoint(state: EndpointFormState): EndpointFormErrors {
    const errors: EndpointFormErrors = {};
    if (state.name.trim().length === 0) {
      errors.name = "Indica un nome per l'endpoint.";
    }
    if (state.base_url.trim().length === 0) {
      errors.base_url = "Indica l'URL di base di llama-server.";
    } else if (!/^https?:\/\//.test(state.base_url.trim())) {
      errors.base_url = "L'URL deve iniziare con http:// o https://.";
    }
    if (state.max_concurrency.trim().length > 0) {
      const parsed = Number.parseInt(state.max_concurrency.trim(), 10);
      if (Number.isNaN(parsed) || parsed < 1) {
        errors.max_concurrency = "Deve essere un intero maggiore di zero.";
      }
    }
    return errors;
  }

  async function handleSaveEndpoint() {
    const errors = validateEndpoint(form);
    setFormErrors(errors);
    setFormError(null);
    if (Object.keys(errors).length > 0) {
      return;
    }

    setSaving(true);
    try {
      const maxConcurrency = form.max_concurrency.trim();
      const saved = await endpointUpsert({
        id: form.id,
        name: form.name.trim(),
        base_url: form.base_url.trim().replace(/\/+$/, ""),
        api_key_ref: form.api_key_ref.trim().length > 0 ? form.api_key_ref.trim() : null,
        max_concurrency: maxConcurrency.length > 0 ? Number.parseInt(maxConcurrency, 10) : null,
        notes: form.notes.trim().length > 0 ? form.notes.trim() : null,
      });
      onSaved(saved);
    } catch (saveError) {
      setFormError(toErrorMessage(saveError));
    } finally {
      setSaving(false);
    }
  }

  return (
    <form
      className="panel"
      onSubmit={(event) => {
        event.preventDefault();
        void handleSaveEndpoint();
      }}
    >
      <div className="panel-head">
        <span className="panel-title">
          {form.id === null ? "Nuovo endpoint" : "Modifica endpoint"}
        </span>
        <button
          type="button"
          className="btn btn-sm btn-ghost"
          onClick={() => {
            onClose();
          }}
        >
          Chiudi
        </button>
      </div>

      <div className="panel-pad section-stack">
        <div className="grid grid-cols-2 gap-3">
          <FormField label="Nome" htmlFor="endpoint-name" required error={formErrors.name}>
            <input
              id="endpoint-name"
              className="input"
              value={form.name}
              onChange={(event) => {
                setForm((current) => ({ ...current, name: event.target.value }));
              }}
              placeholder="traduttore-principale"
            />
          </FormField>

          <FormField
            label="Concorrenza massima"
            htmlFor="endpoint-concurrency"
            hint="Lasciare vuoto per usare gli slot dichiarati dal server."
            error={formErrors.max_concurrency}
          >
            <input
              id="endpoint-concurrency"
              className="input"
              inputMode="numeric"
              value={form.max_concurrency}
              onChange={(event) => {
                setForm((current) => ({ ...current, max_concurrency: event.target.value }));
              }}
              placeholder="4"
            />
          </FormField>
        </div>

        <FormField
          label="URL di base"
          htmlFor="endpoint-url"
          required
          hint="Es. http://127.0.0.1:8080 — su questa base vengono chiamati /health, /props e /v1/models."
          error={formErrors.base_url}
        >
          <input
            id="endpoint-url"
            className="input"
            value={form.base_url}
            spellCheck={false}
            onChange={(event) => {
              setForm((current) => ({ ...current, base_url: event.target.value }));
            }}
          />
        </FormField>

        <div className="grid grid-cols-2 gap-3">
          <FormField
            label="Riferimento API key"
            htmlFor="endpoint-keyref"
            hint="Nome della voce nel keyring di sistema. Il segreto non viene mai salvato nel database."
          >
            <input
              id="endpoint-keyref"
              className="input"
              value={form.api_key_ref}
              spellCheck={false}
              onChange={(event) => {
                setForm((current) => ({ ...current, api_key_ref: event.target.value }));
              }}
              placeholder="llmtranslator/translator"
            />
          </FormField>

          <FormField label="Note" htmlFor="endpoint-notes">
            <input
              id="endpoint-notes"
              className="input"
              value={form.notes}
              onChange={(event) => {
                setForm((current) => ({ ...current, notes: event.target.value }));
              }}
              placeholder="Qwen2.5 32B, -c 32768 --parallel 4"
            />
          </FormField>
        </div>

        {formError !== null ? (
          <div className="banner banner-error" role="alert">
            <span aria-hidden="true">⚠</span>
            <span>{formError}</span>
          </div>
        ) : null}

        <div className="flex items-center gap-2">
          <button type="submit" className="btn btn-primary" disabled={saving}>
            {saving ? <span className="spinner" aria-hidden="true" /> : null}
            {form.id === null ? "Crea endpoint" : "Salva modifiche"}
          </button>
        </div>
      </div>
    </form>
  );
}
