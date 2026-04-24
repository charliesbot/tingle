<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{ label: string; tone?: 'info' | 'warn' }>()
const toneClass = computed(() => props.tone ?? 'info')
</script>

<template>
  <span :class="toneClass">{{ props.label }}</span>
</template>

<style scoped>
span { padding: 4px 8px; }
</style>
