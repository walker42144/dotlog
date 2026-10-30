import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import "./styles/app.css";
import "./styles/desktop.css";
import "./styles/skins.css";
import "./styles/motion.css";

const container = document.getElementById("root");
if (!container) {
  throw new Error("缺少 #root 挂载点");
}

createRoot(container).render(
  <StrictMode>
    <ErrorBoundary><App /></ErrorBoundary>
  </StrictMode>
);
