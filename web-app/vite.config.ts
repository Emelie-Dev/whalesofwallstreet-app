import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  define: {
    __WALLETCONNECT_PROJECT_ID__: JSON.stringify(process.env.VITE_WALLETCONNECT_PROJECT_ID || '3fcc6b1f64f43c3f25c7e090f7777777'),
  },
})
