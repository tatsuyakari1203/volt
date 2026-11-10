import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import './index.css';
import { initLanguage } from './shared/i18n';
import { initTheme } from './shared/theme';

const root = document.getElementById('root');
if (!root) {
  throw new Error('Missing #root element in index.html');
}

// Set data-theme before the first render so there is no flash of the wrong theme.
initTheme();
// The same for the language, so the first paint is already in it.
initLanguage();

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
