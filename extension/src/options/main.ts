import { mount } from "svelte";
import "../tokens.css";
import { followTheme } from "../theme";
import Options from "./Options.svelte";

followTheme();
mount(Options, { target: document.getElementById("app")! });
