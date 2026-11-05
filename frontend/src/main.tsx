import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { CustomerApp } from "./CustomerApp";
import { initLocale, LocalizationProvider } from "./i18n";
import { NavigationProgress } from "./NavigationProgress";
import { SiteFooter } from "./SiteFooter";
import "./styles.css";

// Rendered once the language is known, so the page does not switch under
// the reader. initLocale gives up on the server after two seconds.
void initLocale().then(() =>
  createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <LocalizationProvider>
        {location.pathname.startsWith("/admin") ? <App /> : (
          <div className="flex min-h-screen flex-col">
            <NavigationProgress />
            <CustomerApp />
            <SiteFooter />
          </div>
        )}
      </LocalizationProvider>
    </React.StrictMode>,
  )
);
