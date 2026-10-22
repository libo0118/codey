/** 独立浏览器页面跟随系统外观，并同步后续主题切换。 */
export function installRequestLogTheme(ownerDocument = document) {
  const view = ownerDocument.defaultView!;
  const preference = view.matchMedia("(prefers-color-scheme: dark)");
  const sync = () => {
    const resolved = preference.matches ? "dark" : "light";
    ownerDocument.documentElement.dataset.theme = resolved;
    ownerDocument.documentElement.style.colorScheme = resolved;
  };
  sync();
  preference.addEventListener("change", sync);
  return () => preference.removeEventListener("change", sync);
}
