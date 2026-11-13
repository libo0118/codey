import "react-native-web";

// The 0.19.2 declarations omit these documented react-native-web runtime exports.
declare module "react-native-web" {
  const StyleSheet: import("react-native-web").StyleSheet;
  interface PressableProps { accessibilityRole?: import("react-native").AccessibilityRole }
}
