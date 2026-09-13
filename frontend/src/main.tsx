import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { CustomerApp } from "./CustomerApp";
import { LocalizationProvider } from "./i18n";
import "./styles.css";

const Root = location.pathname.startsWith("/admin") ? App : CustomerApp;
createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <LocalizationProvider>
      <Root />
    </LocalizationProvider>
  </React.StrictMode>,
);
