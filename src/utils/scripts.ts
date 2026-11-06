import { Command } from "@tauri-apps/plugin-shell";
import { invoke } from "@tauri-apps/api/core";

export let powershellArgs = [
    "-NoProfile",
    "-NoLogo",
    "-NonInteractive",
    "-WindowStyle",
    "Hidden",
    "-ExecutionPolicy",
    "Bypass",
    "-Command",
];

export const setCpuPriority = async (pid: number, priority: string) => {
    console.log(pid, priority);
    await invoke("set_cpu_priority", { pid, priority });
};

export const getCpuAffinity = (array: number[]) => {
    const modified = array.map((x: number) => (x = 2 ** x));
    return modified.reduce((total: number, num: number) => total + num, 0);
};

export const bitmaskToCpuArray = (mask: number | undefined): number[] | null => {
    if (mask === undefined || mask < 0) return null;

    const bits: number[] = [];
    let n = BigInt(mask);
    let index = 0;

    while (n > 0n) {
        if ((n & 1n) === 1n) {
            bits.push(index);
        }
        n >>= 1n;
        index++;
    }

    return bits;
};

export function formatAffinity(affinity: number[] | null): string {
    if (affinity === null) return "Protected";
    if (affinity.length === 0) return "All cores";

    const sorted = [...affinity].sort((a, b) => a - b);
    const ranges: string[] = [];
    let start = sorted[0];
    let end = sorted[0];

    for (let i = 1; i <= sorted.length; i++) {
        if (i < sorted.length && sorted[i] === end + 1) {
            end = sorted[i];
        } else {
            ranges.push(start === end ? `${start}` : `${start}-${end}`);
            start = end = sorted[i];
        }
    }

    return ranges.join(", ");
}

export const setProcessAffinity = async (pid: number, affinity: number | string) => {
    const mask = typeof affinity === "string" ? Number(affinity) : affinity;
    if (!Number.isFinite(mask)) return;
    await invoke("set_process_affinity", { pid, affinity: mask });
};

export const setIOPriority = async (pid: number, priority: string) => {
    await invoke("set_io_priority", { pid, priority });
};

export const setMemoryPriority = async (pid: number, priority: string) => {
    await invoke("set_memory_priority", { pid, priority });
};

export const setGPUPriority = async (pid: number, priority: string) => {
    await invoke("set_gpu_priority", { pid, priority });
};

export const setGPUPreference = async (path: string, priority: string) => {
    const args = [
        ...powershellArgs,
        `New-ItemProperty -Path "HKCU:\\Software\\Microsoft\\DirectX\\UserGpuPreferences" -Name "${path}" -Value "${priority}" -PropertyType "String" -Force`,
    ];
    const shell = Command.create("powershell", args);
    await shell.execute();
};

export const terminateProcess = async (pid: number) => {
    await invoke("terminate_process", { pid });
};

export const toggleProcessBoost = async (pid: number, toggle: string) => {
    await invoke("set_process_boost", { pid, enable: toggle === "true" });
};

export const toggleEfficiencyMode = async (pid: number, toggle: string) => {
    await invoke("set_efficiency_mode", { pid, enable: toggle === "true" });
};

export const locateProcess = async (pid: number) => {
    const args = [
        ...powershellArgs,
        `$path = Get-Process -Id ${pid} | Select-Object -ExpandProperty Path; explorer.exe /select,$path`,
    ];
    const shell = Command.create("powershell", args);
    await shell.execute();
};

export const restartProcess = async (pid: number, path: string) => {
    const args = [...powershellArgs, `Stop-Process -Id ${pid}; Start-Process -FilePath "${path}"`];
    const shell = Command.create("powershell", args);
    await shell.execute();
};

export const locatePath = async (path: string) => {
    const args = [...powershellArgs, `explorer.exe /select,${path}`];
    const shell = Command.create("powershell", args);
    await shell.execute();
};

export const runNewProcess = async (path: string) => {
    const args = ["/c", "start", "", path];
    const shell = Command.create("cmd", args);
    await shell.spawn();
};

export const setPowerPlan = async (pid: number, id: string) => {
    await invoke("set_power_plan", { id });
};

export function getCpuCoresArray(): number[] {
    const totalCores = navigator.hardwareConcurrency;
    return Array.from({ length: totalCores }, (_, index) => index);
}

export const getProcessRulesShortcut = async (process: { name: string }, always: any) => {
    let rules = [];
    const allRules = await always.keys();
    const itemRules = allRules.filter((rule: string) => rule.startsWith(`${process.name}_`));
    const entries = await Promise.all(
        itemRules.map(async (rule: string) => {
            const stored = await always.get(rule);
            if (!stored) return null;

            if (stored.type === "boost" || stored.type === "efficiency_mode") {
                if (stored.value === "true") {
                    stored.value = "";
                    stored.type = stored.type.toUpperCase();
                } else {
                    stored.value = "";
                    stored.type = stored.type.toLowerCase();
                }

                return stored.type.charAt(0);
            }

            if (stored.type === "cpu_affinity") {
                return formatAffinity(stored.cores);
            }

            const typeFirstLetter = stored.type.charAt(0);
            const valueFirstLetter = stored.value.charAt(0);
            const shortcut = typeFirstLetter + valueFirstLetter.toUpperCase();

            return shortcut;
        }),
    );
    rules = entries.filter(
        (entry): entry is { key: string; type: string; value: string } => entry !== null,
    );
    return rules;
};

function formatRegistryPath(registry: string) {
    return registry.replace(/^(HKCU|HKLM|HKCR|HKU|HKCC)(?!:)/i, "$1:");
}

export const regDel = async (path: string, item: string) => {
    const args = [
        ...powershellArgs,
        `Remove-ItemProperty -Path "${formatRegistryPath(path)}" -Name "${item}" -Force`,
    ];
    const shell = Command.create("powershell", args);
    await shell.execute();
};

export const toggleStartupEntry = async (
    name: string | null,
    source: string | null,
    registry: string | null,
    command: string | null,
    state: string | null,
) => {
    if (!name || !source || !command || !state) return;

    if (source !== "Registry") {
        const baseDir = command.split("\\Programs\\")[0];
        const parentPath = baseDir + "\\Programs\\Startup";
        let disabledFolder = `${parentPath}\\WinProcessDisabled`;

        async function createDisabledFolder() {
            const args = [
                ...powershellArgs,
                `if (!(Test-Path "${disabledFolder}")) { (New-Item -ItemType Directory -Path "${disabledFolder}" -Force).Attributes = 'Hidden' }`,
            ];
            const shell = Command.create("powershell", args);
            await shell.execute();
        }

        async function disableEntry(path: string) {
            const args = [
                ...powershellArgs,
                `Move-Item -Path "${path}" -Destination "${disabledFolder}" -Force`,
            ];
            const shell = Command.create("powershell", args);
            await shell.execute();
        }

        async function enableEntry(path: string) {
            const args = [
                ...powershellArgs,
                `Move-Item -Path "${path}" -Destination "${parentPath}" -Force`,
            ];
            const shell = Command.create("powershell", args);
            await shell.execute();
        }

        if (state === "true") {
            await createDisabledFolder();
            await disableEntry(command);
        }

        if (
            command.includes("WinProcessDisabled") ||
            (command.includes("AutorunsDisabled") && state === "false")
        ) {
            await enableEntry(command);
        }
    }

    if (!registry) return;

    async function createDisabledKey(path: string) {
        const args = [
            ...powershellArgs,
            `if (!(Test-Path "${path}")) { New-Item -Path "${formatRegistryPath(path)}\\WinProcessDisabled" -Force } `,
        ];
        const shell = Command.create("powershell", args);
        await shell.execute();
    }

    async function regAdd(path: string, item: string, command: string) {
        const args = [
            ...powershellArgs,
            `New-ItemProperty -Path "${formatRegistryPath(path)}" -Name "${item}" -Value "${command}" -PropertyType "String" -Force`,
        ];
        const shell = Command.create("powershell", args);
        await shell.execute();
    }

    if (state === "true") {
        await createDisabledKey(registry);
        await regDel(registry, name);
        await regAdd(registry + "\\WinProcessDisabled", name, command);
    }

    if (registry.includes("WinProcessDisabled") && state === "false") {
        const parentKey = formatRegistryPath(registry.replace("\\WinProcessDisabled", ""));
        await regDel(registry, name);
        await regAdd(parentKey, name, command);
    }

    if (registry.includes("AutorunsDisabled") && state === "false") {
        await regDel(formatRegistryPath(registry), name);
        await regAdd("HKCU:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run", name, command);
    }
};

export const toggleScheduledTask = async (
    name: string | null,
    path: string | null,
    state: string | null,
) => {
    if (!name || !path || !state) return;
    const lastIndex = path.lastIndexOf("\\");
    const parentPath = path.slice(0, lastIndex + 1);

    if (state === "false") {
        const args = [
            ...powershellArgs,
            `Enable-ScheduledTask -TaskPath "${parentPath}" -TaskName "${name}"`,
        ];
        const shell = Command.create("powershell", args);
        await shell.execute();
    } else {
        const args = [
            ...powershellArgs,
            `Disable-ScheduledTask -TaskPath "${parentPath}" -TaskName "${name}"`,
        ];
        const shell = Command.create("powershell", args);
        await shell.execute();
    }
};

export const actionScheduledTask = async (
    name: string | null,
    path: string | null,
    state: string | null,
    status: string | null,
) => {
    if (!name || !path || !state) return;
    const lastIndex = path.lastIndexOf("\\");
    const parentPath = path.slice(0, lastIndex + 1);
    if (state === "true" && status !== "Ready") {
        const args = [
            ...powershellArgs,
            `Stop-ScheduledTask -TaskPath "${parentPath}" -TaskName "${name}"`,
        ];
        const shell = Command.create("powershell", args);
        await shell.execute();
    } else {
        const args = [
            ...powershellArgs,
            `Start-ScheduledTask -TaskPath "${parentPath}" -TaskName "${name}"`,
        ];
        const shell = Command.create("powershell", args);
        await shell.execute();
    }
};

export const deleteScheduledTask = async (name: string | null, path: string | null) => {
    if (!name || !path) return;
    const lastIndex = path.lastIndexOf("\\");
    const parentPath = path.slice(0, lastIndex + 1);
    const args = [
        ...powershellArgs,
        `Unregister-ScheduledTask -TaskPath "${parentPath}" -TaskName "${name}" -Confirm:$false`,
    ];
    const shell = Command.create("powershell", args);
    await shell.execute();
};
