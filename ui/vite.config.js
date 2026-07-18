import { defineConfig, createLogger } from 'vite';
import vue from '@vitejs/plugin-vue';

const host = process.env.TAURI_DEV_HOST;

// Keep Vite's "Local:" URL out of the terminal so Cursor's IDE browser
// doesn't auto-open it; the Tauri window loads this server instead.
const logger = createLogger();
const originalInfo = logger.info.bind(logger);
logger.info = (msg, options) => {
  if (typeof msg === 'string' && /Local:\s+https?:\/\//i.test(msg)) return;
  originalInfo(msg, options);
};

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  customLogger: logger,
  server: {
    host: host || false,
    port: 5173,
    strictPort: true,
    open: false,
    hmr: host ? { protocol: 'ws', host, port: 5183 } : undefined,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
});
