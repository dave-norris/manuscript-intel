import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { ReportTypeDef } from '../types';

const reportTypes = ref<ReportTypeDef[]>([]);
const loaded = ref(false);

async function loadReportTypes(): Promise<void> {
  if (loaded.value) return;
  try {
    reportTypes.value = await invoke<ReportTypeDef[]>('list_report_types_cmd');
    loaded.value = true;
  } catch (e) {
    console.error('Failed to load report types:', e);
  }
}

/**
 * Get the dependants for a report — the reports that must also be selected/deselected.
 * This is just the depends_on array from the DB record.
 */
function getDependants(id: string): string[] {
  const def = reportTypes.value.find(r => r.id === id);
  return def ? def.depends_on : [];
}

export function useReportTypes() {
  return {
    reportTypes,
    loadReportTypes,
    getDependants,
  };
}
