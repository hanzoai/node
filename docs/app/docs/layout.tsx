import { DocsLayout } from 'fumadocs-ui/layout';
import type { ReactNode } from 'react';
import { pageTree } from '@/app/source';
import 'fumadocs-ui/style.css';

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <DocsLayout
      tree={pageTree}
      nav={{
        title: 'Hanzo Node',
        url: '/',
        githubUrl: 'https://github.com/hanzoai/hanzo-node',
      }}
      sidebar={{
        defaultOpenLevel: 1,
        banner: (
          <div className="p-4 bg-blue-50 dark:bg-blue-950 rounded-lg">
            <p className="text-sm">
              <strong>Hanzo Node v1.0</strong> - AI Infrastructure Platform
            </p>
          </div>
        ),
      }}
    >
      {children}
    </DocsLayout>
  );
}