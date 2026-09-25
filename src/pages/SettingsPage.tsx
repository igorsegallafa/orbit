import { ReactNode, useState } from "react";
import { Config } from "../types/config";
import { ReposSection } from "../components/ReposSection";
import { GroupsSection } from "../components/GroupsSection";
import { AiSection } from "../components/AiSection";
import { HealthSection } from "../components/HealthSection";
import { UpdatesSection } from "../components/UpdatesSection";
import { LanguagesSection } from "../components/LanguagesSection";
import { AppearanceSection } from "../components/AppearanceSection";
import {
  ArrowLeftIcon,
  CodeViewIcon,
  DownloadIcon,
  LayersIcon,
  PaletteIcon,
  PulseIcon,
  RepoIcon,
  SearchIcon,
  SparkIcon,
} from "../components/Icons";

export type SettingsSectionId = "repos" | "groups" | "ai" | "languages" | "appearance" | "health" | "updates";

interface SectionDef {
  id: SettingsSectionId;
  title: string;
  description: string;
  icon: ReactNode;
  /** Extra words the nav search matches besides the title. */
  keywords: string;
}

const NAV: { title: string; sections: SectionDef[] }[] = [
  {
    title: "Workspace",
    sections: [
      {
        id: "repos",
        title: "Repositories",
        description: "The repositories Orbit manages and the folders it clones them into.",
        icon: <RepoIcon size={15} />,
        keywords: "repos folders clone path services github",
      },
      {
        id: "groups",
        title: "Groups",
        description: "Named sets of repositories you can spin up together as one workspace.",
        icon: <LayersIcon size={15} />,
        keywords: "groups sets bundle",
      },
    ],
  },
  {
    title: "Tools",
    sections: [
      {
        id: "ai",
        title: "AI",
        description: "The agent and model Orbit uses for sessions, commit messages and reviews.",
        icon: <SparkIcon size={15} />,
        keywords: "agent model claude codex prompt",
      },
      {
        id: "languages",
        title: "Languages",
        description: "Language servers that power go-to-definition, hovers and diagnostics in the editor.",
        icon: <CodeViewIcon size={15} />,
        keywords: "lsp language server diagnostics editor",
      },
    ],
  },
  {
    title: "System",
    sections: [
      {
        id: "appearance",
        title: "Appearance",
        description: "The app's theme: interface, editor, diffs and terminals.",
        icon: <PaletteIcon size={15} />,
        keywords: "theme dark light color dracula nord tokyo catppuccin solarized",
      },
      {
        id: "health",
        title: "Health",
        description: "Tools Orbit relies on, and whether each one is installed.",
        icon: <PulseIcon size={15} />,
        keywords: "environment doctor install git gh node",
      },
      {
        id: "updates",
        title: "Updates",
        description: "The installed version and new releases.",
        icon: <DownloadIcon size={15} />,
        keywords: "version release upgrade",
      },
    ],
  },
];

const SECTIONS = NAV.flatMap((g) => g.sections);

interface NavProps {
  section: SettingsSectionId;
  onSelect: (id: SettingsSectionId) => void;
  onBack: () => void;
  /** Icon-only, for a sidebar dragged narrow. */
  collapsed?: boolean;
}

/** Settings' own navigation: takes the place of the app sidebar while
 *  Settings is open (Orca-style), grouped with a filter on top. */
export function SettingsNav({ section, onSelect, onBack, collapsed }: NavProps) {
  const [query, setQuery] = useState("");
  const q = query.trim().toLowerCase();
  const groups = NAV.map((g) => ({
    ...g,
    sections: g.sections.filter((s) => !q || `${s.title} ${s.keywords}`.toLowerCase().includes(q)),
  })).filter((g) => g.sections.length);

  return (
    <nav className="settings-nav">
      <button className="nav-item settings-nav-back" onClick={onBack} title="Back to app">
        <span className="nav-icon"><ArrowLeftIcon size={15} /></span>
        {!collapsed && "Back to app"}
      </button>
      {!collapsed && (
        <label className="settings-nav-search">
          <SearchIcon size={13} />
          <input
            value={query}
            placeholder="Search settings"
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") setQuery("");
              if (e.key === "Enter" && groups[0]) onSelect(groups[0].sections[0].id);
            }}
            autoComplete="off"
            spellCheck={false}
          />
        </label>
      )}
      {groups.map((g) => (
        <div key={g.title} className="settings-nav-group">
          {!collapsed && <div className="nav-section">{g.title}</div>}
          {g.sections.map((s) => (
            <button
              key={s.id}
              className={`nav-item ${section === s.id ? "active" : ""}`}
              aria-current={section === s.id ? "page" : undefined}
              onClick={() => onSelect(s.id)}
              title={s.title}
            >
              <span className="nav-icon">{s.icon}</span>
              {!collapsed && s.title}
            </button>
          ))}
        </div>
      ))}
      {!groups.length && <div className="settings-nav-empty">No settings match “{query.trim()}”</div>}
    </nav>
  );
}

interface Props {
  config: Config;
  onChange: (cfg: Config) => void;
  onError: (msg: string) => void;
  section: SettingsSectionId;
}

export function SettingsPage({ config, onChange, onError, section }: Props) {
  const def = SECTIONS.find((s) => s.id === section) ?? SECTIONS[0];

  return (
    <div className="page settings-page" key={def.id}>
      <header className="settings-head">
        <h2>{def.title}</h2>
        <p>{def.description}</p>
      </header>

      {def.id === "repos" && <ReposSection config={config} onChange={onChange} onError={onError} />}
      {def.id === "groups" && <GroupsSection config={config} onChange={onChange} onError={onError} />}
      {def.id === "ai" && <AiSection onError={onError} />}
      {def.id === "languages" && <LanguagesSection onError={onError} />}
      {def.id === "appearance" && <AppearanceSection />}
      {def.id === "health" && <HealthSection onError={onError} />}
      {def.id === "updates" && <UpdatesSection />}
    </div>
  );
}
