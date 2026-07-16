import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { ReportEnvelope, SidebarReportGroup } from '../types';

const sidebarGroups = ref<SidebarReportGroup[]>([]);
const currentReport = ref<ReportEnvelope | null>(null);

async function loadSidebarReports(folder: string, platform: string): Promise<void> {
  if (!folder) {
    sidebarGroups.value = [];
    return;
  }
  try {
    sidebarGroups.value = await invoke<SidebarReportGroup[]>('get_sidebar_reports', { folder, platform });
  } catch (e) {
    console.error('loadSidebarReports:', e);
    sidebarGroups.value = [];
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
    sidebarGroups,
    currentReport,
    loadSidebarReports,
    openReport,
    deleteReport,
    closeReport,
  };
}
