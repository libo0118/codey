// Cindy HomeNewTaskButton.tsx:40-48, deef17354321527fd857acf62662955139ef2491,
// Apache-2.0. Retains the floating circular action; no native morph/haptics.
import { Pressable } from "react-native-web";
import { IconEdit } from "@tabler/icons-react";
import { styles } from "./styles";

export function HomeNewTaskButton({ onPress, disabled }: { onPress: () => void; disabled: boolean }) {
  return <Pressable dataSet={{ cindyControl: "" }} accessibilityRole="button" aria-label="新建会话" disabled={disabled}
    onPress={onPress} style={({ pressed }) => [styles.floatingAction, pressed && { opacity: 0.8 }, disabled && styles.disabled]}
    testID="cindy.newChatButton"><IconEdit size={25} stroke={1.7} color="var(--remote-on-accent)" /></Pressable>;
}
