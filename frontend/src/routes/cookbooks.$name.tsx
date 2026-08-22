import { createFileRoute, notFound } from "@tanstack/react-router";
import { useState } from "react";
import { FileCode } from "lucide-react";
import { CodeBlock, KpiCard, MetaGrid, PageHeader, Panel } from "@/components/spindle/ui-bits";
import { cookbookByName } from "@/lib/mock/data";
import { relTime } from "@/lib/format";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/cookbooks/$name")({
  loader: ({ params }) => {
    const cb = cookbookByName(params.name);
    if (!cb) throw notFound();
    return { name: cb.name, description: cb.description };
  },
  head: ({ loaderData }) => {
    if (!loaderData) {
      return { meta: [{ title: "Cookbook not found — Spindle" }, { name: "robots", content: "noindex" }] };
    }
    const title = `${loaderData.name} cookbook — Spindle`;
    return {
      meta: [
        { title },
        { name: "description", content: loaderData.description },
        { property: "og:title", content: title },
        { property: "og:description", content: loaderData.description },
      ],
    };
  },
  component: CookbookDetail,
});

function CookbookDetail() {
  const { name } = Route.useParams();
  const cookbook = cookbookByName(name)!;
  const [versionIdx, setVersionIdx] = useState(0);
  const version = cookbook.versions[versionIdx]!;
  const [filePath, setFilePath] = useState(version.files[0]!.path);
  const file = version.files.find((f) => f.path === filePath) ?? version.files[0]!;

  return (
    <div className="space-y-5">
      <PageHeader
        breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Cookbooks", to: "/cookbooks" }, { label: cookbook.name }]}
        title={<span className="num">{cookbook.name}</span>}
        description={cookbook.description}
      />

      <div className="grid gap-3 sm:grid-cols-3">
        <KpiCard label="Latest version" value={<span className="num">{cookbook.versions[0]!.version}</span>} sub={`${cookbook.versions.length} uploaded`} />
        <KpiCard label="Nodes applying" value={cookbook.nodes} sub="current version" />
        <KpiCard label="Last applied" value={relTime(cookbook.lastSeen)} sub="fleet-wide" />
      </div>

      <div className="panel p-4">
        <MetaGrid
          items={[
            { label: "Maintainer", value: cookbook.maintainer },
            { label: "Versions", value: <span className="num">{cookbook.versions.map((v) => v.version).join(", ")}</span> },
            { label: "License", value: "Apache-2.0" },
            { label: "Source", value: <span className="num">git@spindle.io:cookbooks/{cookbook.name}</span> },
          ]}
        />
      </div>

      <div className="grid gap-4 lg:grid-cols-4">
        <Panel title="Versions" bodyClassName="p-0" className="lg:col-span-1">
          <ul>
            {cookbook.versions.map((v, i) => (
              <li key={v.version}>
                <button
                  onClick={() => {
                    setVersionIdx(i);
                    setFilePath(cookbook.versions[i]!.files[0]!.path);
                  }}
                  className={cn(
                    "flex w-full items-center justify-between gap-2 border-b border-border/60 px-4 py-2.5 text-left transition-colors last:border-0 hover:bg-accent/40",
                    i === versionIdx && "bg-accent/60",
                  )}
                >
                  <span className="num text-xs">{v.version}</span>
                  <span className="num text-[11px] text-muted-foreground">{v.nodes} nodes</span>
                </button>
              </li>
            ))}
          </ul>
        </Panel>

        <Panel
          className="lg:col-span-3"
          title={`Files · v${version.version}`}
          description={`Uploaded ${relTime(version.updatedAt)}`}
          bodyClassName="p-4"
        >
          <div className="mb-3 flex flex-wrap gap-1.5">
            {version.files.map((f) => (
              <button
                key={f.path}
                onClick={() => setFilePath(f.path)}
                className={cn(
                  "num inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-[11px] transition-colors",
                  f.path === filePath
                    ? "border-primary/40 bg-accent text-accent-foreground"
                    : "border-border text-muted-foreground hover:text-foreground",
                )}
              >
                <FileCode className="size-3" />
                {f.path}
              </button>
            ))}
          </div>
          <CodeBlock content={file.content} />
        </Panel>
      </div>
    </div>
  );
}
