// Typed injection keys — compile-time safety for provide/inject.

import type { InjectionKey, Ref, ComputedRef } from 'vue';
import type {
  Story,
  StoriesResult,
  AnalysisState,
  ReportEnvelope,
  SidebarReportGroup,
  Series,
  LogLine,
  Finding,
  ModelInfo,
} from './types';
import type { ContinuityScope } from './composables/useAnalysis';
import type { ModelAssignments, FolderStructure, ThemeMode } from './composables/useSettings';

// ── Stories ───────────────────────────────────────────────────────────────────

export interface StoriesContext {
  stories: Ref<Story[]>;
  activeStoryId: Ref<string | null>;
  activeStory: ComputedRef<Story | null>;
  activeFolder: ComputedRef<string>;
  loadStories: () => Promise<void>;
  setActiveStory: (id: string | null) => void;
  addStory: (name: string, folder: string) => Promise<StoriesResult>;
  initStory: (name: string, parentFolder: string) => Promise<StoriesResult>;
  updateStory: (id: string, name: string, folder: string, biblePath?: string) => Promise<StoriesResult>;
  deleteStory: (id: string) => Promise<StoriesResult>;
}

export const storiesKey: InjectionKey<StoriesContext> = Symbol('stories');

// ── Analysis ──────────────────────────────────────────────────────────────────

export interface AnalysisContext {
  analysisState: Ref<AnalysisState | null>;
  isWorking: Ref<boolean>;
  logLines: Ref<LogLine[]>;
  refreshState: (folder: string) => Promise<void>;
  runAnalyze: (folder: string, forceResummarize: boolean, platform: string) => Promise<void>;
  runCraftAnalysis: (folder: string, selected: string[], continuityScope: ContinuityScope) => Promise<void>;
  runMarketIntel: (folder: string) => Promise<void>;
  cancelOperation: () => Promise<void>;
  clearLog: () => void;
  appendLog: (msg: string) => void;
}

export const analysisKey: InjectionKey<AnalysisContext> = Symbol('analysis');

// ── Platform ──────────────────────────────────────────────────────────────────

export interface PlatformContext {
  platform: Ref<'kdp' | 'wide' | 'craft'>;
  isKdp: ComputedRef<boolean>;
  setPlatform: (p: 'kdp' | 'wide' | 'craft') => void;
}

export const platformKey: InjectionKey<PlatformContext> = Symbol('platform');

// ── Settings ──────────────────────────────────────────────────────────────────

export interface SettingsContext {
  theme: Ref<ThemeMode>;
  setTheme: (mode: ThemeMode) => void;
  provider: Ref<string>;
  apiKey: Ref<string>;
  model: ComputedRef<string>;
  proseModel: ComputedRef<string>;
  modelAssignments: Ref<ModelAssignments>;
  modelFor: (fn: keyof ModelAssignments) => string;
  canopyApiKey: Ref<string>;
  dataforseoLogin: Ref<string>;
  dataforseoPassword: Ref<string>;
  models: Ref<ModelInfo[]>;
  folderStructure: Ref<FolderStructure>;
  fetchModels: () => Promise<{ success: boolean; error: string }>;
  loadFolderStructure: () => Promise<void>;
  addFolderEntry: () => void;
  removeFolderEntry: (index: number) => void;
  saveSettings: () => Promise<void>;
  testCanopy: () => Promise<{ success: boolean; error: string }>;
  testDataforseo: () => Promise<{ success: boolean; error: string }>;
}

export const settingsKey: InjectionKey<SettingsContext> = Symbol('settings');

// ── Reports ───────────────────────────────────────────────────────────────────

export interface ReportsContext {
  sidebarGroups: Ref<SidebarReportGroup[]>;
  currentReport: Ref<ReportEnvelope | null>;
  loadSidebarReports: (folder: string, platform: string) => Promise<void>;
  openReport: (id: number) => Promise<ReportEnvelope>;
  deleteReport: (id: number) => Promise<void>;
  closeReport: () => void;
}

export const reportsKey: InjectionKey<ReportsContext> = Symbol('reports');

// ── Series ────────────────────────────────────────────────────────────────────

export interface SeriesContext {
  series: Ref<Series[]>;
  loadSeries: () => Promise<void>;
}

export const seriesKey: InjectionKey<SeriesContext> = Symbol('series');

// ── Panel navigation ──────────────────────────────────────────────────────────

export const showPanelKey: InjectionKey<(name: string) => void> = Symbol('showPanel');

// ── Manuscript editor ─────────────────────────────────────────────────────────

export const openManuscriptEditorKey: InjectionKey<(findings: Finding[], startIndex: number) => void> = Symbol('openManuscriptEditor');
