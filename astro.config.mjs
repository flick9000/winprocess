// @ts-check
import { defineConfig } from "astro/config";

export default defineConfig({
    vite: {
        clearScreen: false,
        server: {
            port: 4321,
            strictPort: true,

            watch: {
                ignored: ["**/src-tauri/**"],
            },
        },
        envPrefix: ["VITE_", "TAURI_ENV_*"],
        build: {
            target: "chrome105",
            minify: !process.env.TAURI_ENV_DEBUG,
            sourcemap: !!process.env.TAURI_ENV_DEBUG,
        },
    },
});
