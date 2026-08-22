import { useMemo, useState, type ReactNode } from "react";
import { ArrowDown, ArrowUp, ChevronsUpDown, Check, ListFilter, Search, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

export interface Column<T> {
  key: string;
  header: string;
  cell: (row: T) => ReactNode;
  sortValue?: (row: T) => string | number;
  className?: string;
  headerClassName?: string;
  sortable?: boolean;
}

export interface FilterDef {
  id: string;
  label: string;
  options: string[];
  selected: string[];
  onChange: (next: string[]) => void;
}

function FilterMenu({ filter }: { filter: FilterDef }) {
  const [query, setQuery] = useState("");
  const opts = filter.options.filter((o) => o.toLowerCase().includes(query.toLowerCase()));
  const toggle = (o: string) =>
    filter.onChange(
      filter.selected.includes(o) ? filter.selected.filter((s) => s !== o) : [...filter.selected, o],
    );
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="outline"
          size="sm"
          className={cn("h-8 gap-1.5 border-dashed text-xs", filter.selected.length && "border-solid border-primary/40 bg-accent/40")}
        >
          <ListFilter className="size-3.5" />
          {filter.label}
          {filter.selected.length > 0 && (
            <span className="num ml-0.5 rounded bg-primary/15 px-1 text-[11px] text-primary">
              {filter.selected.length}
            </span>
          )}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-56">
        <DropdownMenuLabel className="text-xs">{filter.label}</DropdownMenuLabel>
        <div className="px-2 pb-1.5">
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter values…"
            className="h-7 text-xs"
          />
        </div>
        <DropdownMenuSeparator />
        <div className="scroll-thin max-h-56 overflow-y-auto">
          {opts.length === 0 && <div className="px-2 py-3 text-xs text-muted-foreground">No matches</div>}
          {opts.map((o) => (
            <DropdownMenuItem
              key={o}
              onSelect={(e) => {
                e.preventDefault();
                toggle(o);
              }}
              className="justify-between text-xs capitalize"
            >
              <span className="truncate">{o}</span>
              {filter.selected.includes(o) && <Check className="size-3.5 text-primary" />}
            </DropdownMenuItem>
          ))}
        </div>
        {filter.selected.length > 0 && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => filter.onChange([])} className="text-xs text-muted-foreground">
              Clear selection
            </DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function DataTable<T>({
  columns,
  rows,
  getRowKey,
  searchText,
  searchPlaceholder = "Search…",
  filters = [],
  pageSize = 10,
  onRowClick,
  initialSort,
  emptyTitle = "Nothing to show",
  emptyDescription = "Try clearing filters or widening your search.",
  loading = false,
  toolbarRight,
  density = "normal",
}: {
  columns: Column<T>[];
  rows: T[];
  getRowKey: (row: T) => string;
  searchText?: (row: T) => string;
  searchPlaceholder?: string;
  filters?: FilterDef[];
  pageSize?: number;
  onRowClick?: (row: T) => void;
  initialSort?: { key: string; dir: "asc" | "desc" };
  emptyTitle?: string;
  emptyDescription?: string;
  loading?: boolean;
  toolbarRight?: ReactNode;
  density?: "normal" | "compact";
}) {
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<{ key: string; dir: "asc" | "desc" } | null>(initialSort ?? null);
  const [page, setPage] = useState(1);

  const filtered = useMemo(() => {
    let out = rows;
    if (query && searchText) {
      const q = query.toLowerCase();
      out = out.filter((r) => searchText(r).toLowerCase().includes(q));
    }
    if (sort) {
      const col = columns.find((c) => c.key === sort.key);
      if (col?.sortValue) {
        out = [...out].sort((a, b) => {
          const av = col.sortValue!(a);
          const bv = col.sortValue!(b);
          const cmp = typeof av === "number" && typeof bv === "number" ? av - bv : String(av).localeCompare(String(bv));
          return sort.dir === "asc" ? cmp : -cmp;
        });
      }
    }
    return out;
  }, [rows, query, sort, columns, searchText]);

  const pageCount = Math.max(1, Math.ceil(filtered.length / pageSize));
  const current = Math.min(page, pageCount);
  const visible = filtered.slice((current - 1) * pageSize, current * pageSize);
  const activeFilters = filters.filter((f) => f.selected.length > 0);

  const toggleSort = (key: string) =>
    setSort((prev) =>
      prev?.key === key ? { key, dir: prev.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" },
    );

  const rowPad = density === "compact" ? "h-9" : "h-11";

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        {searchText && (
          <div className="relative min-w-56 flex-1 sm:max-w-72">
            <Search className="absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setPage(1);
              }}
              placeholder={searchPlaceholder}
              className="h-8 pl-8 text-xs"
            />
            {query && (
              <button
                onClick={() => setQuery("")}
                className="absolute top-1/2 right-2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
                aria-label="Clear search"
              >
                <X className="size-3.5" />
              </button>
            )}
          </div>
        )}
        {filters.map((f) => (
          <FilterMenu key={f.id} filter={f} />
        ))}
        {activeFilters.length > 0 && (
          <Button
            variant="ghost"
            size="sm"
            className="h-8 text-xs text-muted-foreground"
            onClick={() => activeFilters.forEach((f) => f.onChange([]))}
          >
            Reset
          </Button>
        )}
        <div className="ml-auto flex items-center gap-2">{toolbarRight}</div>
      </div>

      {activeFilters.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5">
          {activeFilters.flatMap((f) =>
            f.selected.map((s) => (
              <button
                key={`${f.id}-${s}`}
                onClick={() => f.onChange(f.selected.filter((x) => x !== s))}
                className="inline-flex items-center gap-1 rounded-full border border-border bg-elevated px-2 py-0.5 text-[11px] text-muted-foreground transition-colors hover:text-foreground"
              >
                <span className="text-foreground/70">{f.label}:</span>
                <span className="capitalize">{s}</span>
                <X className="size-3" />
              </button>
            )),
          )}
        </div>
      )}

      <div className="panel overflow-hidden">
        <div className="scroll-thin overflow-x-auto">
          <table className="w-full min-w-full text-sm">
            <thead>
              <tr className="border-b border-border bg-elevated/60">
                {columns.map((c) => (
                  <th
                    key={c.key}
                    className={cn("label-caps px-3 py-2 text-left font-medium select-none", c.headerClassName)}
                  >
                    {c.sortable === false || !c.sortValue ? (
                      c.header
                    ) : (
                      <button
                        onClick={() => toggleSort(c.key)}
                        className="inline-flex items-center gap-1 transition-colors hover:text-foreground"
                      >
                        {c.header}
                        {sort?.key === c.key ? (
                          sort.dir === "asc" ? (
                            <ArrowUp className="size-3" />
                          ) : (
                            <ArrowDown className="size-3" />
                          )
                        ) : (
                          <ChevronsUpDown className="size-3 opacity-40" />
                        )}
                      </button>
                    )}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {loading &&
                Array.from({ length: 6 }).map((_, i) => (
                  <tr key={`sk-${i}`} className="border-b border-border/60">
                    {columns.map((c) => (
                      <td key={c.key} className="px-3 py-3">
                        <Skeleton className="h-3.5 w-full max-w-28" />
                      </td>
                    ))}
                  </tr>
                ))}
              {!loading &&
                visible.map((row) => (
                  <tr
                    key={getRowKey(row)}
                    onClick={onRowClick ? () => onRowClick(row) : undefined}
                    className={cn(
                      "border-b border-border/60 last:border-0",
                      rowPad,
                      onRowClick && "cursor-pointer transition-colors hover:bg-accent/40",
                    )}
                  >
                    {columns.map((c) => (
                      <td key={c.key} className={cn("px-3 py-2 align-middle", c.className)}>
                        {c.cell(row)}
                      </td>
                    ))}
                  </tr>
                ))}
              {!loading && visible.length === 0 && (
                <tr>
                  <td colSpan={columns.length} className="px-3 py-14 text-center">
                    <p className="text-sm font-medium text-foreground">{emptyTitle}</p>
                    <p className="mt-1 text-xs text-muted-foreground">{emptyDescription}</p>
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>

      <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
        <span>
          {filtered.length === 0 ? "0" : `${(current - 1) * pageSize + 1}–${Math.min(current * pageSize, filtered.length)}`}{" "}
          of <span className="num text-foreground">{filtered.length}</span>
          {rows.length !== filtered.length && ` (${rows.length} total)`}
        </span>
        <div className="flex items-center gap-1">
          <Button
            variant="outline"
            size="sm"
            className="h-7 px-2 text-xs"
            disabled={current <= 1}
            onClick={() => setPage(current - 1)}
          >
            Previous
          </Button>
          <span className="num px-2">
            {current} / {pageCount}
          </span>
          <Button
            variant="outline"
            size="sm"
            className="h-7 px-2 text-xs"
            disabled={current >= pageCount}
            onClick={() => setPage(current + 1)}
          >
            Next
          </Button>
        </div>
      </div>
    </div>
  );
}
