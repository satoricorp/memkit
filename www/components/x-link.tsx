'use client';

import { siteConfig } from "@/config";

export default function XLink() {
  const link = siteConfig.social.find((item) => item.label.toLowerCase() === "x");

  return (
    <a
      href={link?.href ?? "https://x.com"}
      target="_blank"
      rel="noopener noreferrer"
      className="group relative isolate flex items-center gap-x-2 font-medium text-foreground hover:text-foreground/70 transition-colors"
    >
      <span className="absolute inset-0 -z-10 scale-75 rounded-lg bg-white/5 opacity-0 transition group-hover:scale-100 group-hover:opacity-100" />
      <svg
        viewBox="0 0 16 16"
        aria-hidden="true"
        fill="currentColor"
        className="h-4 w-4 flex-none"
      >
        <path d="M9.51762 6.77491L15.3459 0H13.9648L8.90409 5.88256L4.86212 0H0.200195L6.31244 8.89547L0.200195 16H1.58139L6.92562 9.78782L11.1942 16H15.8562L9.51728 6.77491H9.51762ZM7.62588 8.97384L7.00658 8.08805L2.07905 1.03974H4.20049L8.17706 6.72795L8.79636 7.61374L13.9654 15.0075H11.844L7.62588 8.97418V8.97384Z" />
      </svg>
    </a>
  );
}
