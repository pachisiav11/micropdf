// Pages follow the system's light or dark setting.

export function followTheme(): void {
  const light = matchMedia("(prefers-color-scheme: light)");
  const apply = () => (document.documentElement.dataset.theme = light.matches ? "light" : "dark");
  apply();
  light.addEventListener("change", apply);
}
