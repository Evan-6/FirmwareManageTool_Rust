import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// `base: "./"` matters: the packaged app loads the bundle over tauri://, where
// absolute asset paths resolve to nothing.
export default defineConfig({
  plugins: [react()],
  base: "./",
  build: {
    outDir: "dist",
    target: "chrome114",
    emptyOutDir: true,
  },
  server: { port: 5173, strictPort: true },
});
