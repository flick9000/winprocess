export interface KeyboardNavigationOptions {
    rowSelector: string;
    getItemIdentity: (row: HTMLTableRowElement) => Record<string, string | null>;
    nameSelector?: string;
}

export function initKeyboardNavigation({
    rowSelector,
    getItemIdentity,
    nameSelector = "td .name span",
}: KeyboardNavigationOptions): () => void {
    let selectedRows: Record<string, string | null>[] = [];
    let lastKey = "";

    function navigateRows(rows: NodeListOf<HTMLTableRowElement>, keyPressed: string): boolean {
        for (const row of rows) {
            const name = row.querySelector(nameSelector)?.textContent?.toLowerCase();

            const currentIdentity = getItemIdentity(row);

            const alreadySelected = selectedRows.some((selectedItem) =>
                Object.keys(currentIdentity).every(
                    (key) => selectedItem[key] === currentIdentity[key],
                ),
            );

            if (name?.startsWith(keyPressed) && !alreadySelected) {
                row.classList.add("selected");
                row.scrollIntoView({ block: "center" });
                selectedRows.push({ ...currentIdentity, name });
                return true;
            }
        }
        return false;
    }

    function handleKeyDown(event: KeyboardEvent) {
        const target = event.target as HTMLElement | null;
        if (target?.tagName === "INPUT" || target?.tagName === "TEXTAREA") {
            return;
        }

        const keyPressed = event.key.toLowerCase();

        if (keyPressed.length !== 1 || !/[a-z]/i.test(keyPressed)) {
            return;
        }

        const rows = document.querySelectorAll<HTMLTableRowElement>(rowSelector);
        rows.forEach((row) => row.classList.remove("selected"));

        if (keyPressed !== lastKey) {
            selectedRows = [];
        }

        let keyFound = navigateRows(rows, keyPressed);

        if (!keyFound) {
            selectedRows = [];
            keyFound = navigateRows(rows, keyPressed);
        }

        lastKey = keyPressed;
    }

    document.addEventListener("keydown", handleKeyDown);

    return () => {
        document.removeEventListener("keydown", handleKeyDown);
    };
}
