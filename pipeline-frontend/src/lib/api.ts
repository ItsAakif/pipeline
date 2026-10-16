// Backend API base. Relative by default so a single-container (same-origin)
// deploy works with no build-time configuration; the Vite dev server proxies
// /api to the local backend. Set VITE_API_BASE_URL to an absolute URL (e.g.
// https://backend/api) for a split deploy.
export const API_BASE_URL = import.meta.env.VITE_API_BASE_URL || '/api';
