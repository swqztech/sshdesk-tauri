import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { api } from './api'
import App from './App'
import './index.css'

window.api = api

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>
)
