import { ref, computed } from 'vue';

const platform = ref<'kdp' | 'wide' | 'craft'>(
  (localStorage.getItem('platform') as 'kdp' | 'wide' | 'craft') || 'kdp'
);

const isKdp = computed(() => platform.value === 'kdp');

function setPlatform(p: 'kdp' | 'wide' | 'craft'): void {
  platform.value = p;
  localStorage.setItem('platform', p);
}

export function usePlatform() {
  return {
    platform,
    isKdp,
    setPlatform,
  };
}
