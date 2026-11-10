import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import './index.css';
import { initTheme } from './shared/theme';

const root = document.getElementById('root');
if (!root) {
  throw new Error('Missing #root element in index.html');
}

// Set data-theme before the first render so there is no flash of the wrong theme.
initTheme();

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
