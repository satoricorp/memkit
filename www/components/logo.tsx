import Link from "next/link";

export default function Logo() {
  return (
    <Link href="/" className="flex items-center gap-3 group">
      <span className="size-8 rounded-full bg-white" />
      <span className="text-lg font-pixel tracking-tight group-hover:opacity-80">www</span>
    </Link>
  );
}
