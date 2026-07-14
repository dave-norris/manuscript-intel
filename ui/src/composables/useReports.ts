import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { DocMeta, SavedReportMeta, ReportEnvelope } from '../types';

const reports = ref<DocMeta[]>([]);
const savedReports = ref<SavedReportMeta[]>([]);
const currentReport = ref<ReportEnvelope | null>(null);

async function loadReports(folder: string): Promise<void> {
  if (!folder) {
    reports.value = [];
    savedReports.value = [];
    return;
  }
  try {
    const [docs, saved] = await Promise.all([
      invoke<DocMeta[]>('list_reports_cmd', { folder }),
      invoke<SavedReportMeta[]>('list_saved_reports_cmd', { folder }),
    ]);
    reports.value = docs;
    savedReports.value = saved;
  } catch (e) {
    console.error('loadReports:', e);
    reports.value = [];
    savedReports.value = [];
  }
}

async function openReport(folder: string, docType: string): Promise<ReportEnvelope> {
  const envelope = await invoke<ReportEnvelope>('get_report_cmd', { folder, docType });
  currentReport.value = envelope;
  return envelope;
}

async function openSavedReport(id: number): Promise<ReportEnvelope> {
  const envelope = await invoke<ReportEnvelope>('get_saved_report_cmd', { id });
  currentReport.value = envelope;
  return envelope;
}

async function saveVersion(folder: string, docType: string): Promise<SavedReportMeta> {
  const meta = await invoke<SavedReportMeta>('save_report_version_cmd', { folder, docType });
  return meta;
}

async function deleteVersion(id: number): Promise<void> {
  await invoke<void>('delete_saved_report_cmd', { id });
}

function closeReport(): void {
  currentReport.value = null;
}

export function useReports() {
  return {
    reports,
    savedReports,
    currentReport,
    loadReports,
    openReport,
    openSavedReport,
    saveVersion,
    deleteVersion,
    closeReport,
  };
}
