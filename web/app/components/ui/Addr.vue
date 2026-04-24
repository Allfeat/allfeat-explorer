<script setup lang="ts">
// SS58 address display: identicon swatch + shortened address + copy
// button. The identicon is on by default; set `identicon="false"` on
// tight layouts (nested tables) that carry their own avatar.
//
// When the address matches an entry in the known-accounts registry
// (see utils/knownAccounts.ts) the friendly name takes the place of the
// truncated hash; the full SS58 is still copyable and surfaces on hover
// via the `title` attribute.

const props = withDefaults(defineProps<{
  text: string
  to?: string | null
  head?: number
  tail?: number
  copy?: boolean
  identicon?: boolean
  identiconSize?: number
}>(), {
  to: null,
  head: 6,
  tail: 6,
  copy: true,
  identicon: true,
  identiconSize: 18,
})

const short = computed(() => shortAddr(props.text, props.head, props.tail))
const known = useKnownAccount(() => props.text)
</script>

<template>
  <span class="addr-wrap">
    <Identicon v-if="identicon" :seed="text" :size="identiconSize" />
    <template v-if="known">
      <NuxtLink v-if="to" :to="to" class="known-name" :title="text">
        {{ known.name }}
      </NuxtLink>
      <span v-else class="known-name" :title="text">{{ known.name }}</span>
      <span class="known-kind">{{ known.kind }}</span>
    </template>
    <template v-else>
      <NuxtLink v-if="to" :to="to" class="hash">{{ short }}</NuxtLink>
      <span v-else class="hash">{{ short }}</span>
    </template>
    <CopyButton v-if="copy" :text="text" />
  </span>
</template>

<style scoped>
.addr-wrap {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.known-name {
  font-weight: 600;
  font-size: 12px;
  letter-spacing: 0.01em;
}

a.known-name {
  color: inherit;
  text-decoration: none;
}
a.known-name:hover {
  text-decoration: underline;
}

.known-kind {
  font-size: 10px;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  padding: 1px 6px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.06);
  color: var(--dim, rgba(255, 255, 255, 0.55));
  border: 1px solid rgba(255, 255, 255, 0.08);
}
</style>
