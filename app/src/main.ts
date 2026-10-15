import { mount } from "svelte";
// Bundled with the app (no network): Mona Sans for the interface, Hubot Sans for figures.
import "@fontsource-variable/mona-sans/standard.css";
import "@fontsource-variable/hubot-sans/standard.css";
import "./style.css";
import App from "./App.svelte";

mount(App, { target: document.getElementById("app")! });
