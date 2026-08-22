# Spindle Frontend

A static single-page application (SPA) built with React 19, TanStack Router,
TanStack Query, and Tailwind CSS v4. It is served as static files from the
`dist/` directory.

## Build & Development

```bash
# Install dependencies (bun is required)
bun install

# Development server
bun dev

# Production build
bun run build

# Preview the production build
bun run preview
```

## Architecture

- **Routing**: `@tanstack/react-router` (client-side, no SSR)
- **Data fetching**: `@tanstack/react-query` — all routes use `useQuery` hooks
  that call the Spindle HTTP API (`/v1/...`)
- **Styling**: Tailwind CSS with custom oklch design tokens
- **Build**: Vite (outputs to `dist/`)

## API Client

All API communication goes through `src/lib/api.ts`. The API token is stored
in `localStorage` under the key `spindle_token` and sent as a Bearer token.

## Conventions

- Never import from `@/lib/mock/data` — that file has been deleted. Mock data
  is not used in this application.
- Type definitions live in `@/lib/mock/types` (the file name is historical;
  it contains real type contracts, not mock data).
- Every route must handle `isLoading` and `error` states explicitly.
- All data is fetched via `useQuery` from the real Spindle API.
