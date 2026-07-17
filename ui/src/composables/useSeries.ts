import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { Series, SeriesResult } from '../types';

const seriesList = ref<Series[]>([]);

async function loadSeries(): Promise<void> {
  try {
    const result = await invoke<SeriesResult>('list_series');
    seriesList.value = result.success ? result.series : [];
  } catch (e) {
    console.error('loadSeries:', e);
    seriesList.value = [];
  }
}

async function createSeries(name: string, books: { story_folder: string; story_name: string; book_order: number }[]): Promise<SeriesResult> {
  const result = await invoke<SeriesResult>('create_series', { request: { name, books } });
  if (result.success) seriesList.value = result.series;
  return result;
}

async function updateSeries(id: number, name: string, books: { story_folder: string; story_name: string; book_order: number }[], biblePath: string = ''): Promise<SeriesResult> {
  const result = await invoke<SeriesResult>('update_series', { request: { id, name, books, bible_path: biblePath } });
  if (result.success) seriesList.value = result.series;
  return result;
}

async function deleteSeries(id: number): Promise<SeriesResult> {
  const result = await invoke<SeriesResult>('delete_series', { id });
  if (result.success) seriesList.value = result.series;
  return result;
}

export function useSeries() {
  return {
    series: seriesList,
    loadSeries,
    createSeries,
    updateSeries,
    deleteSeries,
  };
}
