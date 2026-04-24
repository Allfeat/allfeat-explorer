import { fileURLToPath } from 'node:url'

export default defineNuxtConfig({
  srcDir: 'app/',
  ssr: true,

  typescript: {
    strict: true,
    typeCheck: true,
  },

  alias: {
    '@bindings': fileURLToPath(new URL('../bindings/index.ts', import.meta.url)),
  },

  modules: ['@pinia/nuxt', '@vueuse/nuxt', '@nuxtjs/color-mode'],

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
