import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri expects a fixed port and no screen clearing, so its own logs stay visible.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  build: {
    target: "es2022",
    sourcemap: false,
    // Two pages: the island, and the overlay shown while the agent acts on screen.
    rollupOptions: { input: { main: "index.html", overlay: "overlay.html" } },
  },
});
