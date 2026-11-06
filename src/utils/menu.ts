export const STATIC_MENUS = {
    cpu: [
        { type: "cpu_priority", id: "realtime", label: "Realtime" },
        { type: "cpu_priority", id: "high", label: "High" },
        { type: "cpu_priority", id: "abovenormal", label: "Above Normal" },
        { type: "cpu_priority", id: "normal", label: "Normal" },
        { type: "cpu_priority", id: "belownormal", label: "Below Normal" },
        { type: "cpu_priority", id: "idle", label: "Idle" },
    ],
    gpu: [
        { type: "gpu_priority", id: "realtime", label: "Realtime" },
        { type: "gpu_priority", id: "high", label: "High" },
        { type: "gpu_priority", id: "abovenormal", label: "Above Normal" },
        { type: "gpu_priority", id: "normal", label: "Normal" },
        { type: "gpu_priority", id: "belownormal", label: "Below Normal" },
        { type: "gpu_priority", id: "idle", label: "Idle" },
    ],
    gpu_preference: [
        {
            type: "gpu_preference",
            id: "GpuPreference=0;",
            label: "System Default",
        },
        {
            type: "gpu_preference",
            id: "GpuPreference=1;",
            label: "Power Saving",
        },
        {
            type: "gpu_preference",
            id: "GpuPreference=2;",
            label: "High Performance",
        },
    ],
    io: [
        { type: "io_priority", id: "high", label: "High" },
        { type: "io_priority", id: "normal", label: "Normal" },
        { type: "io_priority", id: "low", label: "Low" },
        { type: "io_priority", id: "verylow", label: "Very Low" },
    ],
    mem: [
        { type: "mem_priority", id: "normal", label: "Normal" },
        { type: "mem_priority", id: "belownormal", label: "Below Normal" },
        { type: "mem_priority", id: "medium", label: "Medium" },
        { type: "mem_priority", id: "low", label: "Low" },
        { type: "mem_priority", id: "verylow", label: "Very Low" },
    ],
    efficiency: [
        { type: "efficiency_mode", id: "true", label: "Enabled" },
        { type: "efficiency_mode", id: "false", label: "Disabled" },
    ],
    boost: [
        { type: "boost", id: "true", label: "Enabled" },
        { type: "boost", id: "false", label: "Disabled" },
    ],
};

export const setMenuPosition = async (menuElement: HTMLElement | null, event: MouseEvent) => {
    event.preventDefault();
    if (!menuElement) return;

    const menuWidth = menuElement.offsetWidth;
    const menuHeight = menuElement.offsetHeight;
    const windowWidth = window.innerWidth;
    const windowHeight = window.innerHeight;

    let x = event.clientX;
    let y = event.clientY;
    if (x + menuWidth > windowWidth - 300) {
        x = x - menuWidth;
        menuElement.classList.add("left");
    } else {
        menuElement.classList.remove("left");
    }
    if (y + menuHeight > windowHeight) {
        y = windowHeight - menuHeight - 31;
    }
    menuElement.style.left = `${x}px`;
    menuElement.style.top = `${y}px`;

    menuElement.classList.add("active");
};
