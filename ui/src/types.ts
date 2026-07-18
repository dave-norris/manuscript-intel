// ── Shared types for Manuscript Intel UI ──────────────────────────────────────

export interface Story {
  id: string;
  name: string;
  folder: string;
  created: string;
  bible_path: string;
}

export interface StoriesResult {
  success: boolean;
  stories: Story[];
  error: string;
}

export interface GenreResult {
  success: boolean;
  report: string;
  error: string;
  run_ts: string;
}

export interface ModelInfo {
  id: string;
  owned_by: string;
  input_price: number | null;
  output_price: number | null;
}

export interface ModelsResult {
  success: boolean;
  models: ModelInfo[];
  error: string;
}

export interface AnalysisState {
  has_folder: boolean;
  summary_count: number;
  has_genre_data: boolean;
  has_full_report: boolean;
  has_keywords: boolean;
  has_search_terms: boolean;
  has_competition: boolean;
  has_categories: boolean;
  has_genre_ranking: boolean;
  has_mapped_verified: boolean;
  has_bisac: boolean;
  has_discovery_keywords: boolean;
  has_keyword_search_results: boolean;
  has_zeigarnik: boolean;
  has_continuity_check: boolean;
  has_show_dont_tell: boolean;
  has_ai_isms: boolean;
}

export interface SeriesRow {
  id: number;
  name: string;
  book_count: number;
}

export interface SeriesBookRow {
  story_folder: string;
  story_name: string;
  book_order: number;
}

export interface Settings {
  provider: string;
  apiKey: string;
  model: string;
  canopyApiKey: string;
}

export interface DocMeta {
  id: number;
  doc_type: string;
  label: string;
  generated_at: string;
}

export interface SavedReportMeta {
  id: number;
  doc_type: string;
  version: number;
  label: string;
  saved_at: string;
}

export interface ReportEnvelope {
  id: number;
  doc_type: string;
  label: string;
  format: string;
  content: string;
  generated_at: string;
}

export interface LogLine {
  type: string;
  icon: string;
  text: string;
}

export interface SidebarReportVersion {
  id: number;
  generated_at: string;
}

export interface SidebarReportGroup {
  doc_type: string;
  label: string;
  description: string;
  count: number;
  versions: SidebarReportVersion[];
}

export interface Finding {
  filePath: string;
  chapterTitle: string;
  tellingText: string;
  context: string;
  why: string;
  severity: string;
  reportType: 'show_dont_tell' | 'ai_isms' | 'continuity';
  entity?: string;
  attribute?: string;
  explanation?: string;
  occurrences?: { story_name: string; file: string; chapter_title: string; value: string; snippet: string }[];
}

export interface WinningCatImportResult {
  success: boolean;
  imported: number;
  skipped_other_department: number;
  skipped_unparseable: number;
  stale_count: number;
  imported_at: string;
  error: string;
}

export interface StaleCleanupResult {
  success: boolean;
  removed: number;
  error: string;
}

export interface ReportTypeDef {
  id: string;
  label: string;
  description: string;
  platforms: string[];
  depends_on: string[];
  model_slot: string;
  min_tier: string;
}

export interface SeriesBook {
  story_folder: string;
  story_name: string;
  book_order: number;
}

export interface Series {
  id: number;
  name: string;
  created_at: string;
  bible_path: string;
  books: SeriesBook[];
}

export interface SeriesResult {
  success: boolean;
  series: Series[];
  error: string;
}
