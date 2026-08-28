import './global.css';
import { RootProvider } from 'fumadocs-ui/provider';
import { Zen } from '@hanzo/font'
import type { ReactNode } from 'react';

const sans = Zen

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" className={sans.className} suppressHydrationWarning>
      <body className="flex flex-col min-h-screen">
        <RootProvider>{children}</RootProvider>
      </body>
    </html>
  );
}

export const metadata = {
  title: 'Hanzo Node Documentation',
  description: 'AI Infrastructure Platform with 100+ LLM Providers, Tool Execution, and Job Orchestration',
  keywords: 'AI, LLM, Infrastructure, Tool Execution, Job Orchestration, LanceDB, Vector Search',
};