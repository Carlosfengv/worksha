import React from "react";
import ReactDOM from "react-dom/client";
import App from "./app/App";
import { renderErrorOptions } from "./app/render-boundary";
import "./styles/globals.css";

ReactDOM.createRoot(
  document.getElementById("root") as HTMLElement,
  renderErrorOptions,
).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
