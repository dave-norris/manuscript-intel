import { ref, computed } from 'vue';

export const KDP_REPORT_TYPES = new Set([
  'analysis', 'genre_analysis', 'full_report', 'kdp_keywords', 'mi_search_terms',
  'competition_report', 'category_finder', 'genre_ranking', 'bisac_classification',
  'review_mining', 'author_analysis', 'chapter_summaries', 'keyword_search',
  'mapped_categories', 'genres_and_categories',
]);

export const WIDE_REPORT_TYPES = new Set([
  'genre_analysis', 'bisac_classification', 'discovery_keywords',
  'genre_ranking', 'chapter_summaries',
]);

const platform = ref<'kdp' | 'wide'>(
  (localStorage.getItem('platform') as 'kdp' | 'wide') || 'kdp'
);

const isKdp = computed(() => platform.value === 'kdp');

function setPlatform(p: 'kdp' | 'wide'): void {
  platform.value = p;
  localStorage.setItem('platform', p);
}

export function usePlatform() {
  return {
    platform,
    isKdp,
    setPlatform,
    KDP_REPORT_TYPES,
    WIDE_REPORT_TYPES,
  };
}
