import Nav from "@/components/nav";
import Footer from "@/components/footer";
import { siteConfig, marketingConfig } from "@/config";

export default function AboutPage() {
  return (
    <>
      <Nav />
      <main className="mx-auto flex min-h-screen w-full max-w-4xl flex-col items-center justify-center gap-10 px-6 py-24 text-center">
        <div className="space-y-4">
          <div className="text-xs uppercase tracking-[0.3em] text-muted-foreground">
            About {siteConfig.brand.name}
          </div>
          <h1 className="text-4xl font-display md:text-6xl">
            {siteConfig.brand.tagline}
          </h1>
          <p className="text-sm text-muted-foreground md:text-base">
            {siteConfig.brand.description}
          </p>
        </div>
        <div className="grid gap-6 md:grid-cols-2">
          {marketingConfig.sections.map((section) => (
            <div key={section.id} className="rounded-2xl border border-border/70 bg-background/70 p-6 text-left">
              <h2 className="text-lg font-brand">{section.title}</h2>
              <p className="mt-2 text-sm text-muted-foreground">{section.body}</p>
            </div>
          ))}
        </div>
      </main>
      <Footer />
    </>
  );
}
