import { invoke } from "@tauri-apps/api/core";

export async function fetchAndApplyIcon(
    exePath: string,
    imageElement: HTMLImageElement,
    iconCache: Map<string, string>,
) {
    if (!exePath || !imageElement) return;

    if (iconCache.has(exePath)) {
        imageElement.src = iconCache.get(exePath)!;
        return;
    }

    const icon = await invoke<string | null>("get_process_icon", {
        exePath: exePath,
    });

    if (icon) {
        iconCache.set(exePath, icon);
        imageElement.src = icon;
    }
}
