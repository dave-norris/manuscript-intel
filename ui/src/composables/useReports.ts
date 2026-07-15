import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { DocMeta, ReportEnvelope } from '../types';

const reports = ref<DocMeta[]>([]);
const currentReport = ref<ReportEnvelope | null>(null);

async function loadReports(folder: string): Promise<void> {
  if (!folder) {
    reports.value = [];
    return;
  }
  try {
    const docs = await invoke<DocMeta[]>('list_reports_cmd', { folder });
    reports.value = docs;
  } catch (e) {
    console.error('loadReports:', e);
    reports.value = [];
  }
}

async function openReport(id: number): Promise<ReportEnvelope> {
  const envelope = await invoke<ReportEnvelope>('get_report_cmd', { id });
  currentReport.value = envelope;
  return envelope;
}

async function deleteReport(id: number): Promise<void> {
  await invoke<void>('delete_report_cmd', { id });
}

function closeReport(): void {
  currentReport.value = null;
}

export function useReports() {
  return {
    reports,
    currentReport,
    loadReports,
    openReport,
    deleteReport,
    closeReport,
  };
}
