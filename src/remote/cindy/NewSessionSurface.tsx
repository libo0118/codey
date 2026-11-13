// Adapted Cindy apps/mobile/app/sessions/new.tsx:5160-5187,5269-5287,
// 5617-5671,6320-6343 at deef17354321527fd857acf62662955139ef2491, Apache-2.0.
import { useState, type ReactNode } from "react";
import { Pressable, ScrollView, Text, View } from "react-native-web";
import { IconArrowLeft, IconCheck, IconDeviceLaptop, IconFolder, IconSelector } from "@tabler/icons-react";
import type { Project } from "../workspace-data";
import { styles } from "./styles";

export function NewSessionSurface({ projects, project, projectless, busy, defaultsReady, onProject, onBack, children, notices }: {
  projects: Project[]; project?: Project; projectless: boolean; busy: boolean; defaultsReady: boolean;
  onProject: (project: Project | null) => void; onBack: () => void; children: ReactNode; notices: ReactNode;
}) {
  const [workspacePickerOpen, setWorkspacePickerOpen] = useState(false);
  return <main className="cindy-new-session">
    <header className="cindy-create-header" data-codey-chrome="header">
      <button className="cindy-icon-button" data-codey-action="back" aria-label="返回会话列表" disabled={busy} onClick={onBack}><IconArrowLeft size={22} /></button><h2>新建对话</h2><span />
    </header>
    <ScrollView style={styles.fill} contentContainerStyle={styles.bottomCluster}>
      <div className="cindy-create-intro"><h1>从一个想法开始</h1><p>首条消息发送后创建对话。</p></div>
      <View style={styles.selectorStack}>
        <View style={styles.selectorRow}><IconDeviceLaptop size={20} className="cindy-session-icon" /><Text style={styles.selectorText} numberOfLines={1}>{typeof location === "undefined" ? "当前连接电脑" : location.host}</Text></View>
        <View>
          <Pressable dataSet={{ cindyControl: "" }} aria-label="选择工作区" accessibilityRole="button" aria-expanded={workspacePickerOpen} aria-disabled={busy} disabled={busy}
            onPress={() => setWorkspacePickerOpen(value => !value)} style={({ pressed }) => [styles.selectorRow, pressed && styles.pressed, busy && styles.disabled]}>
            <IconFolder size={20} className="cindy-session-icon" />
            <View style={styles.selectorLabel}><Text style={styles.selectorText} numberOfLines={1}>{projectless ? "不指定工作目录" : project?.name || "没有可用项目"}</Text><Text style={styles.workspacePath} numberOfLines={1}>{projectless ? "由 Codex 管理实际目录，仍按所选权限执行" : project?.cwd || "可选择不指定工作目录，或在电脑保存本地项目"}</Text></View><IconSelector size={16} />
          </Pressable>
          {workspacePickerOpen && <div className="cindy-workspace-options" role="group" aria-label="工作区">
            <Pressable dataSet={{ cindyControl: "" }} accessibilityRole="button" aria-label="不指定工作目录" aria-pressed={projectless} aria-disabled={busy} disabled={busy}
              onPress={() => { onProject(null); setWorkspacePickerOpen(false); }} style={({ pressed }) => [styles.workspaceOption, pressed && styles.pressed]}>
              <IconDeviceLaptop size={18} /><View style={styles.selectorLabel}><Text style={styles.selectorText}>不指定工作目录</Text><Text style={styles.workspacePath}>由 Codex 管理实际目录</Text></View>{projectless && <IconCheck size={18} />}
            </Pressable>
            {projects.map(item => <div title={item.cwd} key={item.id}><Pressable dataSet={{ cindyControl: "" }} accessibilityRole="button" aria-label={`选择项目 ${item.name}`} aria-pressed={project?.id === item.id} aria-disabled={busy} disabled={busy}
              onPress={() => { onProject(item); setWorkspacePickerOpen(false); }} style={({ pressed }) => [styles.workspaceOption, pressed && styles.pressed]}>
              <IconFolder size={18} /><View style={styles.selectorLabel}><Text style={styles.selectorText} numberOfLines={1}>{item.name}</Text><Text style={styles.workspacePath} numberOfLines={1}>{item.cwd}</Text></View>{project?.id === item.id && <IconCheck size={18} />}
            </Pressable></div>)}
          </div>}
        </View>
        {(project || projectless) && !defaultsReady && <p className="cindy-defaults-status" role="status">默认设置尚未就绪；准备完成后才能发送。</p>}
      </View>
    </ScrollView>
    <div className="cindy-create-dock">{notices}{children}</div>
  </main>;
}
