export type CodeGraphIndex = {
  initialized: boolean;
  projectPath: string;
  fileCount?: number;
  nodeCount?: number;
  edgeCount?: number;
  lastIndexed?: string | null;
  pendingChanges?: { added: number; modified: number; removed: number };
  index?: { state?: string | null; reindexRecommended?: boolean; pendingRefs?: number; filesWithParseErrors?: number };
};

export type BuiltinToolsStatus = {
  installed: boolean;
  message?: string;
  version: string;
  mcp: { id?: string; enabled: boolean; managed: boolean; automatic: boolean };
  job: { id: string; phase: "running" | "succeeded" | "failed"; message: string } | null;
};

export function indexNeedsRepair(index: CodeGraphIndex): boolean {
  return Boolean(index.index?.reindexRecommended) || ["partial", "failed", "indexing"].includes(index.index?.state ?? "");
}

export function indexSummary(index: CodeGraphIndex): string {
  if (!index.initialized) return "尚未建立索引";
  if (indexNeedsRepair(index)) return "索引需要重建";
  const changes = index.pendingChanges;
  const pending = changes ? changes.added + changes.modified + changes.removed : 0;
  if (pending || index.index?.pendingRefs) return "有变更待同步";
  if (index.index?.filesWithParseErrors) return "部分文件解析失败";
  return "索引已就绪";
}
