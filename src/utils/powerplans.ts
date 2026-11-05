import { invoke } from "@tauri-apps/api/core";

interface PowerPlan {
    id: string;
    name: string;
    active: boolean;
}

export async function loadPowerPlans(container: Element | null) {
    try {
        if (!container) return;
        const plans = await invoke<PowerPlan[]>("get_power_plans");
        const dropdown = container?.querySelector("ul");
        if (!dropdown) return;
        dropdown?.classList.add("always");

        dropdown.innerHTML = plans
            .map(
                (plan) =>
                    `<li class="menu-item"><button class="nested-button" data-type="powerplan" data-value="${plan.id}">${plan.name}</button><div class="checked"><img width="16" src="/check.svg" alt="" /></div></li>`,
            )
            .join("");
    } catch (error) {
        console.error(error);
    }
}
