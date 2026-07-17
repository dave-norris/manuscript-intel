import { createApp } from 'vue'
import App from './App.vue'
import './style.css'

const app = createApp(App);

app.config.errorHandler = (err, _instance, info) => {
  console.error('[Vue Error]', info, err);
  // Show a non-blocking notification rather than crashing silently
  const el = document.getElementById('error-toast');
  if (el) {
    el.textContent = `Error: ${err instanceof Error ? err.message : String(err)}`;
    el.style.display = 'block';
    setTimeout(() => { el.style.display = 'none'; }, 5000);
  }
};

app.mount('#app');
