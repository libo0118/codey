// Adapted Cindy HomeSurface.tsx:4213-4289,4320-4344,4744-4782,4841-4868,
// deef17354321527fd857acf62662955139ef2491 (Apache-2.0). See SOURCE.md.
import { useMemo, useState } from "react";
import { Pressable, ScrollView, Text, View } from "react-native-web";
import { IconArchive, IconBrandOpenai, IconChevronDown, IconChevronRight, IconDeviceLaptop, IconDots, IconFolder, IconFolderOpen, IconLoader2, IconLogout, IconPlus, IconRefresh, IconSettings } from "@tabler/icons-react";
import { groupThreads, relativeTime, type Project, type Thread } from "../workspace-data";
import { CopyButton, sessionDeepLink } from "../clipboard";
import { HomeNewTaskButton } from "./HomeNewTaskButton";
import { styles } from "./styles";

export type HomeSurfaceProps = {
  threads: Thread[]; projects: Project[]; selectedId?: string; busy: boolean; loading: boolean;
  archived: boolean; error: string; showHeaderChrome: boolean;
  onArchived: (archived: boolean) => void;
  onOpen: (thread: Thread) => void; onCreate: (project?: Project) => void;
  onRefresh: () => void; onPanel: () => void; onLogout: () => void;
};

export function HomeSessionRow({ thread, selected, disabled, onOpen }: { thread: Thread; selected: boolean; disabled: boolean; onOpen: (thread: Thread) => void }) {
  const link = sessionDeepLink(thread.id);
  const running = thread.running === true;
  return <div className="cindy-session-row" title={thread.title} data-selected={selected || undefined}>
    <Pressable dataSet={{ cindyControl: "" }} accessibilityRole="button" aria-label={thread.title} aria-current={selected ? "page" : undefined} aria-disabled={disabled} disabled={disabled}
      onPress={() => onOpen(thread)} style={({ pressed }) => [styles.sessionRow, pressed && styles.pressed, disabled && styles.disabled]}>
      <View style={styles.sessionIconCell}><IconBrandOpenai size={18} stroke={1.5} className={running ? "cindy-running-icon" : "cindy-session-icon"}
        role={running ? "img" : undefined} aria-label={running ? "正在进行" : undefined} aria-hidden={running ? undefined : true} /></View>
      <View style={styles.sessionListContent}><View style={styles.sessionTitleRow}>
        <Text style={styles.sessionTitle} numberOfLines={1}>{thread.title}</Text>
        {running ? <View style={styles.sessionRightStatusCell}><IconLoader2 size={15} className="cindy-running-spinner" aria-hidden="true" /></View>
          : <Text style={styles.sessionTime}><time dateTime={new Date(thread.updatedAt || 0).toISOString()}>{relativeTime(thread.updatedAt)}</time></Text>}
      </View></View>
    </Pressable>
    {link && <details className="cindy-session-menu" onKeyDown={event => { if (event.key === "Escape") { event.currentTarget.open = false; event.currentTarget.querySelector("summary")?.focus(); } }}>
      <summary aria-label={`${thread.title}的会话操作`}><IconDots size={16} /></summary><div><CopyButton text={link} label="复制会话链接" /></div>
    </details>}
  </div>;
}

export function HomeSurface(props: HomeSurfaceProps) {
  const [collapsed, setCollapsed] = useState(new Set<string>());
  const [expanded, setExpanded] = useState(new Set<string>());
  const { groups, recent } = useMemo(() => groupThreads(props.threads, props.projects), [props.threads, props.projects]);
  const toggle = (setter: typeof setCollapsed, id: string) => setter(current => {
    const next = new Set(current); if (next.has(id)) next.delete(id); else next.add(id); return next;
  });
  const row = (thread: Thread) => <HomeSessionRow key={thread.id} thread={thread} selected={props.selectedId === thread.id} disabled={props.busy} onOpen={props.onOpen} />;
  return <aside className="cindy-home" aria-label="项目和会话">
    <header className="cindy-home-header" data-codey-chrome={props.showHeaderChrome ? "header" : undefined}>
      <View style={styles.header}><h1>Codex</h1></View>
      <p className="cindy-host"><IconDeviceLaptop size={14} /><span title={typeof location === "undefined" ? undefined : location.host}>{typeof location === "undefined" ? "当前连接电脑" : location.host}</span></p>
    </header>
    <div className="cindy-home-body" aria-busy={props.loading}>
      <ScrollView style={styles.fill} contentContainerStyle={{ paddingHorizontal: 12, paddingBottom: 90 }}>
        <div className="cindy-section-heading"><h2>{props.archived ? "归档项目" : "项目"}</h2><button className="cindy-icon-button" aria-label="刷新会话" onClick={props.onRefresh} disabled={props.busy || props.loading}><IconRefresh size={16} /></button></div>
        {groups.map(({ project, threads }) => {
          const open = !collapsed.has(project.id);
          const all = expanded.has(project.id);
          const running = !open && threads.some(thread => thread.running === true);
          return <section className="cindy-project" key={project.id}>
            <div className="cindy-project-header" title={project.cwd}>
              <Pressable dataSet={{ cindyControl: "" }} accessibilityRole="button" aria-label={`项目 ${project.name}`} aria-expanded={open}
                onPress={() => toggle(setCollapsed, project.id)} style={({ pressed }) => [styles.projectRow, pressed && styles.pressed]}>
                {open ? <IconChevronDown size={16} /> : <IconChevronRight size={16} />}
                {open ? <IconFolderOpen size={19} /> : <IconFolder size={19} className={running ? "cindy-running-icon" : undefined} />}
                <View style={styles.projectLabel}><Text style={styles.projectTitle} numberOfLines={1}>{project.name}</Text></View>
                <Text style={styles.count} aria-label={`当前已加载 ${threads.length} 个会话`}>{threads.length}</Text>
                {running && <View style={styles.sessionRightStatusCell} aria-label="已加载会话中有任务正在进行"><IconLoader2 size={15} className="cindy-running-spinner" aria-hidden="true" /></View>}
              </Pressable>
              <button className="cindy-icon-button" aria-label={`在 ${project.name} 中新建会话`} disabled={props.busy} onClick={() => props.onCreate(project)}><IconPlus size={17} /></button>
            </div>
            {open && <div className="cindy-project-children">{(all ? threads : threads.slice(0, 5)).map(row)}
              {!threads.length && <p className="cindy-project-empty">此项目暂无{props.archived ? "归档" : ""}会话</p>}
              {threads.length > 5 && <button className="cindy-show-more" onClick={() => toggle(setExpanded, project.id)}>{all ? "收起" : `展开显示 (${threads.length - 5})`}</button>}
            </div>}
          </section>;
        })}
        <div className="cindy-section-heading cindy-recent-heading"><h2>{props.archived ? "归档会话" : "最近"}</h2><small>计数仅含当前已加载</small></div>
        {recent.map(row)}
        {!props.loading && !props.threads.length && <p className="cindy-list-empty">{props.archived ? "暂无归档会话" : "选择项目，开始新的对话"}</p>}
        {props.loading && !props.threads.length && <p className="cindy-list-empty" role="status">正在加载会话…</p>}
        {props.error ? <p role="alert" className="remote-client-error">{props.error}</p> : null}
      </ScrollView>
      <HomeNewTaskButton onPress={() => props.onCreate()} disabled={props.busy} />
    </div>
    <footer className="cindy-home-footer" data-codey-chrome="footer">
      <button data-codey-action="archive" aria-pressed={props.archived} disabled={props.busy} onClick={() => props.onArchived(!props.archived)}><IconArchive size={17} />{props.archived ? "返回会话" : "归档会话"}</button>
      <button data-codey-action="console" disabled={props.busy} onClick={props.onPanel}><IconSettings size={17} />Codey 控制台</button>
      <button data-codey-action="logout" className="cindy-icon-button" aria-label="退出此设备" disabled={props.busy} onClick={props.onLogout}><IconLogout size={17} /></button>
    </footer>
  </aside>;
}
