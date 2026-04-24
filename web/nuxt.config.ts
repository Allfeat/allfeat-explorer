import { execSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { readFileSync } from 'node:fs'

// Frontend build stamp. CI / Docker builds can inject NUXT_PUBLIC_GIT_SHA
// directly (no `.git` in the image); local builds fall back to the
// current HEAD via `git rev-parse`. Version comes from package.json so
// bumping one place keeps everything in sync.
const frontendVersion = (JSON.parse(
  readFileSync(new URL('./package.json', import.meta.url), 'utf8'),
) as { version?: string }).version ?? '0.0.0'

const frontendGitSha = process.env.NUXT_PUBLIC_GIT_SHA
  ?? (() => {
    try {
      return execSync('git rev-parse --short=7 HEAD', { stdio: ['ignore', 'pipe', 'ignore'] })
        .toString().trim()
    } catch {
      return 'unknown'
    }
  })()

export default defineNuxtConfig({
  srcDir: 'app/',
  ssr: true,

  typescript: {
    strict: true,
    // Opt-out via `NUXT_TYPECHECK=false` in Docker/CI builds: the vue-tsc
    // fork that `typeCheck: true` spawns keeps an IPC channel open past
    // the "Build complete!" point in non-tty runners, wedging the image
    // build. Local `bun run build` keeps the check on by default.
    typeCheck: process.env.NUXT_TYPECHECK !== 'false',
  },

  alias: {
    '@bindings': fileURLToPath(new URL('../bindings/index.ts', import.meta.url)),
  },

  modules: ['@pinia/nuxt', '@vueuse/nuxt', '@nuxtjs/color-mode'],

  // Override the SWR cache storage in dev to the in-memory driver. The
  // default `fs` driver writes cache entries mirroring the URL path,
  // which collides when a parent route (`/ats`) and its sub-routes
  // (`/ats/[id]`) are both cached — the parent wants `ats` as a file
  // and the children need `ats/` as a directory. Memory storage
  // sidesteps the filesystem entirely and is fine in dev (cache is
  // rebuilt on every restart anyway). Prod still uses the default disk
  // driver via the top-level `storage` key (untouched here).
  nitro: {
    devStorage: {
      cache: {
        driver: 'memory',
      },
    },
  },

  // Nitro route-level stale-while-revalidate. Cache keys include the
  // full URL (including `?network=`), so per-network SSR stays isolated.
  // Listings / dashboards use a short window aligned on block time;
  // detail pages get a longer window because finalised records don't
  // mutate. Nitro serves the cached HTML instantly and regenerates in
  // the background when stale — navigation perceives no latency after
  // the first miss. No page reads cookies at render time, so URL-keyed
  // caching is safe (the color-mode module paints the theme client-side
  // before first frame).
  routeRules: {
    '/': { swr: 3 },
    '/blocks': { swr: 3 },
    '/blocks/**': { swr: 30 },
    '/extrinsics': { swr: 3 },
    '/extrinsics/**': { swr: 30 },
    '/events': { swr: 3 },
    '/accounts': { swr: 5 },
    '/accounts/**': { swr: 15 },
    '/ats': { swr: 5 },
    '/ats/**': { swr: 30 },
    '/token': { swr: 30 },
    '/token/**': { swr: 60 },
    '/runtime': { swr: 60 },
  },

  // Emit `data-theme="dark"|"light"` on <html> so our token CSS (in
  // `assets/styles/_tokens.scss`) picks up the mode. Default preference is
  // 'system' — the module injects a head script that sets the attribute
  // synchronously before first paint, so there's no flash on reload.
  colorMode: {
    preference: 'system',
    fallback: 'dark',
    dataValue: 'theme',
    classSuffix: '',
  },

  // Flat auto-import: components/ui/Hash.vue → <Hash/>, not <UiHash/>.
  // Matches the maquette's naming; collisions across subdirs are a lint
  // concern we'll handle if it ever bites.
  components: [
    { path: '~/components', pathPrefix: false },
  ],

  css: ['~/assets/styles/main.scss'],

  vite: {
    // Tokens are CSS custom properties loaded once from main.scss. We don't
    // inject them via additionalData: that would re-emit the :root{} block
    // inside every scoped .vue style unit (harmless but wasteful, and Vue
    // scopes the selector so the declarations never take effect from there).
    server: {
      // HTTP proxy only — WebSocket upgrades for /api/v1/live are not
      // forwarded by Nuxt's dev server (Nitro/h3 intercepts the upgrade
      // event before middlewares). The live socket targets the backend
      // directly in dev (see runtimeConfig.public.wsBase, Phase 8).
      proxy: {
        '/api': {
          target: 'http://127.0.0.1:8088',
          changeOrigin: true,
        },
      },
    },
  },

  runtimeConfig: {
    // Server-only. $fetch on SSR dispatches internally through Nitro and
    // doesn't honour Vite's dev proxy — so SSR calls must target the
    // backend directly. Override in prod via NUXT_API_ORIGIN.
    apiOrigin: 'http://127.0.0.1:8088',
    public: {
      apiBase: '/api/v1',
      // Direct backend address in dev — used by useLiveSocket because
      // Nuxt's dev server cannot proxy WebSocket upgrades (see vite.server
      // comment above). In prod, override via NUXT_PUBLIC_WS_BASE so the
      // reverse proxy handles both HTTP and WS uniformly.
      wsBase: 'ws://127.0.0.1:8088/api/v1',
      frontendVersion,
      frontendGitSha,
    },
  },

  app: {
    // `mode: 'out-in'` is required: NuxtPage wraps the route component
    // in `<Transition><Suspense>…</Suspense></Transition>`, and Suspense
    // is Transition's direct child across route changes — its identity
    // doesn't flip, so Transition wouldn't observe a child swap without
    // `out-in` and enter/leave would never fire. The trade-off is a
    // brief blank between leave-done and enter-start while the new
    // page's top-level `await useFetch` resolves; `<NuxtLoadingIndicator>`
    // in the layout covers that gap visually.
    pageTransition: { name: 'page', mode: 'out-in' },
    head: {
      htmlAttrs: { lang: 'en' },
      title: 'Allfeat Explorer',
      meta: [
        { charset: 'utf-8' },
        { name: 'viewport', content: 'width=device-width,initial-scale=1' },
        { name: 'theme-color', content: '#0b0d12' },
        // Defaults inherited by every page; per-page `useSeoMeta` overrides
        // title/description/ogTitle/ogDescription but keeps these images.
        { property: 'og:image', content: '/og-v2.png' },
        { property: 'og:image:width', content: '1200' },
        { property: 'og:image:height', content: '630' },
        { property: 'og:site_name', content: 'Allfeat Explorer' },
        { name: 'twitter:card', content: 'summary_large_image' },
        { name: 'twitter:image', content: '/twitter.png' },
      ],
      link: [
        { rel: 'icon', type: 'image/x-icon', href: '/favicon.ico' },
        { rel: 'icon', type: 'image/png', sizes: '512x512', href: '/favicon-512.png' },
        { rel: 'apple-touch-icon', href: '/apple-touch-icon.png' },
        { rel: 'preconnect', href: 'https://fonts.googleapis.com' },
        { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: '' },
        {
          rel: 'stylesheet',
          href: 'https://fonts.googleapis.com/css2?family=Space+Grotesk:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500;600&display=swap',
        },
      ],
    },
  },
})
