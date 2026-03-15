"use client";

import { cn } from "@/lib/utils";
import NextLink from "next/link";

interface GetStartedButtonProps {
  className?: string;
}

export default function GetStartedButton({
  className
}: GetStartedButtonProps) {
  const ArrowIcon = () => (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className="transition-transform duration-200 group-hover:translate-x-1 group-hover:-translate-y-1"
    >
      <path d="M7 7h10v10" />
      <path d="M7 17 17 7" />
    </svg>
  );

  return (
    <NextLink
      href="/signin"
      className={cn(
        "min-w-[150px] px-4 py-2 font-mono text-sm rounded hover:bg-gray-800 transition-colors group flex items-center gap-2",
        className
      )}
    >
      Sign Up
      <ArrowIcon />
    </NextLink>
  );
}
