// Adapted from Cindy apps/mobile/app/sessions/new.tsx:6320-6343 and
// HomeSurface.tsx:5197-5204,5529-5535 at deef17354321527fd857acf62662955139ef2491.
// Apache-2.0; platform animation/theme tokens replaced by Codey's web theme.
import { StyleSheet } from "react-native-web";

export const styles = StyleSheet.create({
  fill: { flex: 1, minHeight: 0, minWidth: 0 },
  header: { alignItems: "center", flexDirection: "row", justifyContent: "space-between", minHeight: 64, paddingHorizontal: 16, paddingVertical: 8 },
  projectRow: { alignItems: "center", flexDirection: "row", gap: 8, minHeight: 44, paddingHorizontal: 8, flex: 1, minWidth: 0 },
  projectLabel: { flex: 1, minWidth: 0 },
  projectTitle: { color: "var(--remote-fg)", fontSize: 14, fontWeight: "500" },
  count: { color: "var(--remote-muted)", fontSize: 11, flexShrink: 0 },
  sessionRow: { alignItems: "center", flexDirection: "row", minHeight: 44, flex: 1, minWidth: 0, paddingHorizontal: 8 },
  sessionIconCell: { alignItems: "center", justifyContent: "center", width: 20, height: 20, marginRight: 8, flexShrink: 0 },
  sessionListContent: { flex: 1, minWidth: 0 },
  sessionTitleRow: { alignItems: "center", flexDirection: "row", gap: 8, minWidth: 0 },
  sessionTitle: { color: "var(--remote-fg)", flex: 1, minWidth: 0, fontSize: 14, lineHeight: 20 },
  sessionTime: { color: "var(--remote-muted)", fontSize: 11, flexShrink: 0 },
  sessionRightStatusCell: { alignItems: "center", flexShrink: 0, height: 18, justifyContent: "center", width: 18 },
  pressed: { backgroundColor: "var(--remote-hover)" },
  disabled: { opacity: 0.45 },
  bottomCluster: { flexGrow: 1, gap: 24, justifyContent: "flex-end", paddingBottom: 16 },
  selectorStack: { gap: 16, paddingHorizontal: 8 },
  selectorRow: { alignItems: "center", flexDirection: "row", gap: 12, minHeight: 44 },
  selectorText: { color: "var(--remote-muted)", flexShrink: 1, fontSize: 15, lineHeight: 22, minWidth: 0 },
  selectorLabel: { flex: 1, minWidth: 0 },
  workspacePath: { color: "var(--remote-muted)", fontSize: 11, lineHeight: 18 },
  workspaceOption: { alignItems: "center", flexDirection: "row", gap: 10, minHeight: 52, paddingHorizontal: 12, paddingVertical: 6 },
  floatingAction: { alignItems: "center", justifyContent: "center", position: "absolute", right: 20, bottom: 18, height: 56, width: 56, borderRadius: 28, backgroundColor: "var(--remote-accent)" },
});
