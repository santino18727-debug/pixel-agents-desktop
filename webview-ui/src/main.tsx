// Based on pixel-agents by pablodelucca (https://github.com/pablodelucca/pixel-agents)
// Licensed under MIT
import './vscode-shim';
import './index.css';

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import App from './App.tsx';
import { PetView } from './components/PetView.tsx';
import { isBrowserRuntime } from './runtime';

const isPetMode = new URLSearchParams(window.location.search).get('petMode') === 'true';

async function main() {
  if (isPetMode) {
    // Mark the document so CSS can make the body transparent.
    document.body.classList.add('pet-mode');
    document.documentElement.classList.add('pet-mode');
    createRoot(document.getElementById('root')!).render(
      <StrictMode>
        <PetView />
      </StrictMode>,
    );
    return;
  }

  if (isBrowserRuntime) {
    const { initBrowserMock } = await import('./browserMock.js');
    await initBrowserMock();
  }
  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

main().catch(console.error);
