// One error surface for the whole app. The headline and hint are what the user
// acts on; the untouched Rust chain lives behind the disclosure so it can be
// copied into a bug report.
import { useState } from "react";

import type { UiError } from "../lib/errors";

function copyText(text: string) {
  if (navigator.clipboard?.writeText) {
    return navigator.clipboard.writeText(text);
  }
  // tauri:// is not a secure context in every WebView2 build, so keep a fallback.
  const area = document.createElement("textarea");
  area.value = text;
  area.style.position = "fixed";
  area.style.opacity = "0";
  document.body.appendChild(area);
  area.select();
  document.execCommand("copy");
  document.body.removeChild(area);
  return Promise.resolve();
}

export function ErrorBanner({ error, onDismiss }: { error: UiError; onDismiss?: () => void }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="banner bad error-banner">
      <div className="error-body">
        <strong>{error.title}</strong>
        {error.hint && <span className="error-hint">{error.hint}</span>}
        {error.detail !== error.title && (
          <details className="error-detail">
            <summary>技術細節{error.source ? `（${error.source}）` : ""}</summary>
            <pre className="mono">{error.detail}</pre>
          </details>
        )}
      </div>
      <div className="error-actions">
        {error.detail !== error.title && (
          <button
            className="small"
            onClick={() => {
              void copyText(`${error.title}\n${error.detail}`).then(() => setCopied(true));
            }}
          >
            {copied ? "已複製" : "複製"}
          </button>
        )}
        {onDismiss && (
          <button className="small" onClick={onDismiss}>
            知道了
          </button>
        )}
      </div>
    </div>
  );
}

/** For panels that keep an error next to the control that caused it. */
export function ErrorLine({ error }: { error: UiError }) {
  return (
    <p className="error-text" title={error.detail}>
      {error.title}
      {error.hint ? `：${error.hint}` : ""}
    </p>
  );
}
