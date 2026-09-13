import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { CustomerApp } from "./CustomerApp";
import "./styles.css";

const Root = location.pathname.startsWith("/admin") ? App : CustomerApp;
createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
