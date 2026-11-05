type SortKey = "name" | "rules" | "cpu" | "priority" | "pid" | "affinity";

export function updateSortIndicators(sortKey: string, sortDir: "asc" | "desc") {
    document.querySelectorAll("[data-sort]").forEach((el) => {
        const key = el.getAttribute("data-sort") as SortKey;
        const arrow = el.querySelector<HTMLSpanElement>(".arrow");
        if (!arrow) return;
        if (key === sortKey) {
            arrow.textContent = sortDir === "asc" ? " ↑" : " ↓";
            arrow.style.opacity = "1";
            el.classList.add("active");
        } else {
            arrow.style.opacity = "0";
            el.classList.remove("active");
        }
    });
}
