import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/archivo/wdth.css";
import { invoke } from "@tauri-apps/api/core";
import { setLocale } from "./i18n";
import App from "./App";

invoke<string>("os_locale")
  .then(setLocale, () => {})
  .finally(() =>
    ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
      <React.StrictMode>
        <App />
      </React.StrictMode>,
    ),
  );
