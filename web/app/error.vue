<script setup lang="ts">
import type { NuxtError } from '#app'

defineProps<{ error: NuxtError }>()

function handleError() {
  clearError({ redirect: '/' })
}
</script>

<template>
  <main class="error">
    <h1>{{ error.statusCode }}</h1>
    <p>{{ error.message }}</p>
    <button type="button" @click="handleError">
      Back home
    </button>
  </main>
</template>

<style scoped lang="scss">
.error {
  display: grid;
  place-content: center;
  min-height: 100vh;
  gap: 1rem;
  text-align: center;
  padding: 2rem;
}
</style>
