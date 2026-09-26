import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";

// Boot invariant: index.html always ships the `#root` node, so this can only fail if the
// bundle and the document got out of sync (developer-only diagnostic, never rendered).
const container = document.getElementById("root");

if (container === null) {
  throw new Error("LocalLLMTranslator: #root container missing from index.html");
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
