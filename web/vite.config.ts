import { defineConfig, loadEnv } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "");
  const apiBase = env.MCPWAY_API_BASE?.trim() || "http://127.0.0.1:5173";

  return {
    plugins: [solid()],
    server: {
      hmr: {
        overlay: false
      },
      proxy: {
        "/api": {
          target: apiBase,
          changeOrigin: true,
          ws: true
        }
      }
    },
    build: {
      target: "es2020"
    }
  };
});
