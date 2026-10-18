import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

// The transparent WebView also needs a transparent document canvas. Apply this
// before React's first paint so the normal panel background never covers the pet.
const isPetWindow =
  new URLSearchParams(window.location.search).get("window") === "pet";
document.documentElement.classList.toggle("pet-mode", isPetWindow);
document.body.classList.toggle("pet-mode", isPetWindow);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
