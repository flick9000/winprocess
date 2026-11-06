export function searchFilter(rows: NodeListOf<HTMLTableRowElement>, currentSearch: string) {
    rows.forEach((row) => {
        const name = row.querySelector("td .name span")?.textContent.toLowerCase() ?? "";
        const matches = name.includes(currentSearch);

        row.style.display = matches ? "" : "none";
    });
}
