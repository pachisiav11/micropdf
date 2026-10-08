import { mount } from "svelte";
import "../tokens.css";
import "./viewer.css";
import App from "./App.svelte";

// Follow the system's light or dark setting.
const light = matchMedia("(prefers-color-scheme: light)");
const theme = () => (document.documentElement.dataset.theme = light.matches ? "light" : "dark");
theme();
light.addEventListener("change", theme);

mount(App, { target: document.getElementById("app")! });
