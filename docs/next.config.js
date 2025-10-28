import { createMDX } from 'fumadocs-mdx/next';

const withMDX = createMDX();

/** @type {import('next').NextConfig} */
const config = {
  reactStrictMode: true,
  basePath: process.env.NODE_ENV === 'production' ? '/hanzo-node-docs' : '',
  assetPrefix: process.env.NODE_ENV === 'production' ? '/hanzo-node-docs' : '',
  images: {
    unoptimized: true,
  },
  output: 'export',
};

export default withMDX(config);