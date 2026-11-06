import { getCurrentWindow } from "@tauri-apps/api/window";
import { check } from "@tauri-apps/plugin-updater";
import { ask } from "@tauri-apps/plugin-dialog";
import { relaunch } from "@tauri-apps/plugin-process";

async function checkAndUpdate() {
    try {
        const update = await check();
        if (!update) return;

        let updateAsk = await ask("An update is available. Do you want to update?", {
            title: "Update Available",
            kind: "info",
            okLabel: "Update",
            cancelLabel: "Later",
        });

        if (updateAsk === true) {
            await update.downloadAndInstall();
            await relaunch();
        } else {
            return;
        }
    } catch (error) {
        console.error(error);
    }
}

checkAndUpdate();

document.addEventListener("contextmenu", (event) => {
    event.preventDefault();
});

const tabs = document.querySelectorAll(".sidebar-entry");
const contents = document.querySelectorAll(".tab-content");
const title = document.getElementById("content-header");

if (!(globalThis as any).currentTab) {
    (globalThis as any).currentTab;
}

if (!(globalThis as any).pollingRate) {
    (globalThis as any).pollingRate;
}

if (tabs.length > 0 && contents.length > 0) {
    tabs[0].classList.add("active");
    contents[0].classList.add("active");
    title!.textContent = tabs[0].textContent || "WinProcess";
}

contents.forEach((content, index) => {
    if (index !== 0) {
        content.classList.remove("active");
    }
});

tabs.forEach((tab, index) => {
    tab.addEventListener("click", () => {
        tabs.forEach((t) => t.classList.remove("active"));
        contents.forEach((c) => c.classList.remove("active"));

        tab.classList.add("active");
        contents[index].classList.add("active");
        title!.textContent = tab.textContent;
        (globalThis as any).currentTab = index;
    });
});

document.querySelector(".collapse")?.addEventListener("click", () => {
    const sidebar = document.querySelector(".sidebar");
    const content = document.querySelector(".content");
    sidebar?.classList.toggle("collapsed");
    content?.classList.toggle("expanded");
});

async function showWindow() {
    const win = getCurrentWindow();
    await win.show();
    await win.setFocus();
}

document.addEventListener("DOMContentLoaded", showWindow);
