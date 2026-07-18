import { ref, computed } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { Story, StoriesResult } from '../types';

const stories = ref<Story[]>([]);
const activeStoryId = ref<string | null>(localStorage.getItem('activeStoryId') || null);

const activeStory = computed<Story | null>(() => {
  return stories.value.find(s => s.id === activeStoryId.value) || null;
});

const activeFolder = computed<string>(() => {
  return activeStory.value?.folder || '';
});

async function loadStories(): Promise<void> {
  const result = await invoke<StoriesResult>('list_stories');
  stories.value = result.success ? result.stories : [];

  // If the stored active story no longer exists, clear it
  if (activeStoryId.value && !stories.value.find(s => s.id === activeStoryId.value)) {
    setActiveStory(null);
  }
}

function setActiveStory(id: string | null): void {
  activeStoryId.value = id;
  localStorage.setItem('activeStoryId', id || '');
}

async function addStory(name: string, folder: string): Promise<StoriesResult> {
  const result = await invoke<StoriesResult>('add_story', { request: { name, folder } });
  if (result.success) {
    stories.value = result.stories;
  }
  return result;
}

async function initStory(name: string, parentFolder: string): Promise<StoriesResult> {
  const result = await invoke<StoriesResult>('init_story', {
    request: { name, parent_folder: parentFolder },
  });
  if (result.success) {
    stories.value = result.stories;
  }
  return result;
}

async function updateStory(id: string, name: string, folder: string, biblePath: string = ''): Promise<StoriesResult> {
  const result = await invoke<StoriesResult>('update_story', { request: { id, name, folder, bible_path: biblePath } });
  if (result.success) {
    stories.value = result.stories;
  }
  return result;
}

async function deleteStory(id: string): Promise<StoriesResult> {
  const result = await invoke<StoriesResult>('delete_story', { id });
  if (result.success) {
    stories.value = result.stories;
    if (activeStoryId.value === id) {
      setActiveStory(null);
    }
  }
  return result;
}

export function useStories() {
  return {
    stories,
    activeStoryId,
    activeStory,
    activeFolder,
    loadStories,
    setActiveStory,
    addStory,
    initStory,
    updateStory,
    deleteStory,
  };
}
