// The overlay page: a see-through layer over the main screen while the agent acts.
import { mount } from "svelte";
import "@fontsource-variable/mona-sans/standard.css";
import "../style.css";
import Overlay from "./Overlay.svelte";

mount(Overlay, { target: document.getElementById("overlay")! });
