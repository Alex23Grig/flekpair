import { ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { parseLinkToken } from "./errors";

export const ExternalLink = ({
  url,
  children,
}: {
  url: string;
  children: ReactNode;
}) => (
  <a
    href={url}
    onClick={(event) => {
      event.preventDefault();
      openUrl(url).catch((e) => console.error("Failed to open link", e));
    }}
  >
    {children}
  </a>
);

/** Text in which `((link:url:label))` becomes a link. */
export const LinkedText = ({ text }: { text: string }) => (
  <>
    {text.split(/(\(\(link:[^)]+\)\))/g).map((part, index) => {
      const link = parseLinkToken(part);
      return link ? (
        <ExternalLink key={index} url={link.url}>
          {link.text}
        </ExternalLink>
      ) : (
        <span key={index}>{part}</span>
      );
    })}
  </>
);

export const Suggestions = ({ items }: { items: string[] }) => (
  <ul className="suggestions">
    {items.map((suggestion) => (
      <li key={suggestion}>
        <LinkedText text={suggestion} />
      </li>
    ))}
  </ul>
);
