import React, { lazy, Suspense } from "react";
import { createRoot } from "react-dom/client";
import { CustomerApp } from "./CustomerApp";
import { initLocale, LocalizationProvider } from "./i18n";
import { NavigationProgress } from "./NavigationProgress";
import { SiteFooter } from "./SiteFooter";
import "./styles.css";

// The operators' console is a page few visitors open; it loads on its own.
const App = lazy(() => import("./App").then((module) => ({ default: module.App })));

// Rendered once the language is known, so the page does not switch under
// the reader. initLocale gives up on the server after two seconds.
void initLocale().then(() =>
  createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <LocalizationProvider>
        {location.pathname.startsWith("/admin") ? <Suspense><App /></Suspense> : (
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
