import { ref, computed } from 'vue';

export const KDP_REPORT_TYPES = new Set([
  'analysis', 'genre_analysis', 'full_report', 'kdp_keywords', 'mi_search_terms',
  'competition_report', 'category_finder', 'genre_ranking', 'bisac_classification',
  'review_mining', 'author_analysis', 'chapter_summaries', 'keyword_search',
  'mapped_categories', 'genres_and_categories', 'activity_log',
]);

export const WIDE_REPORT_TYPES = new Set([
  'genre_analysis', 'bisac_classification', 'discovery_keywords',
  'genre_ranking', 'chapter_summaries', 'activity_log',
]);

export const CRAFT_REPORT_TYPES = new Set([
  'zeigarnik_analysis', 'continuity_check', 'chapter_summaries', 'activity_log',
]);

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
    KDP_REPORT_TYPES,
    WIDE_REPORT_TYPES,
    CRAFT_REPORT_TYPES,
  };
}
