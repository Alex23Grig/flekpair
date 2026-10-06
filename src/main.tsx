import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./i18next";

const render = () => {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
};

// `npm run dev` opened in a plain browser has no backend, so stand one in.
if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
  import("./dev-mock").then(render);
} else {
  render();
}
