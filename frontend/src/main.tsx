import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { CustomerApp } from "./CustomerApp";
import { initLocale, LocalizationProvider } from "./i18n";
import "./styles.css";

const Root = location.pathname.startsWith("/admin") ? App : CustomerApp;
// Rendered once the language is known, so the page does not switch under
// the reader. initLocale gives up on the server after two seconds.
void initLocale().then(() =>
  createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <LocalizationProvider>
        <Root />
      </LocalizationProvider>
    </React.StrictMode>,
  )
);
