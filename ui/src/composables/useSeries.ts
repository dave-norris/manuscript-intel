import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { SeriesRow, SeriesBookRow } from '../types';

const series = ref<SeriesRow[]>([]);
const activeSeriesBooks = ref<SeriesBookRow[]>([]);

async function loadSeries(): Promise<void> {
  try {
    series.value = await invoke<SeriesRow[]>('list_series_cmd');
  } catch (e) {
    console.error('Failed to load series:', e);
  }
}

async function createSeries(name: string): Promise<SeriesRow | null> {
  try {
    const row = await invoke<SeriesRow>('create_series_cmd', { name });
    await loadSeries();
    return row;
  } catch (e) {
    console.error('Failed to create series:', e);
    return null;
  }
}

async function deleteSeries(seriesId: number): Promise<void> {
  try {
    await invoke('delete_series_cmd', { seriesId });
    await loadSeries();
  } catch (e) {
    console.error('Failed to delete series:', e);
  }
}

async function loadSeriesBooks(seriesId: number): Promise<void> {
  try {
    activeSeriesBooks.value = await invoke<SeriesBookRow[]>('list_series_books_cmd', { seriesId });
  } catch (e) {
    console.error('Failed to load series books:', e);
    activeSeriesBooks.value = [];
  }
}

async function addStoryToSeries(seriesId: number, storyFolder: string, storyName: string, bookOrder: number): Promise<void> {
  try {
    await invoke('add_story_to_series_cmd', {
      request: { series_id: seriesId, story_folder: storyFolder, story_name: storyName, book_order: bookOrder },
    });
    await loadSeriesBooks(seriesId);
    await loadSeries();
  } catch (e) {
    console.error('Failed to add story to series:', e);
  }
}

async function removeStoryFromSeries(seriesId: number, storyFolder: string): Promise<void> {
  try {
    await invoke('remove_story_from_series_cmd', { seriesId, storyFolder });
    await loadSeriesBooks(seriesId);
    await loadSeries();
  } catch (e) {
    console.error('Failed to remove story from series:', e);
  }
}

export function useSeries() {
  return {
    series,
    activeSeriesBooks,
    loadSeries,
    createSeries,
    deleteSeries,
    loadSeriesBooks,
    addStoryToSeries,
    removeStoryFromSeries,
  };
}
