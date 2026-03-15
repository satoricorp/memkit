"use client";

import Image from "next/image";
import Link from "next/link";
import { useRef, useState } from "react";
import { useAuthToken } from "@convex-dev/auth/react";
import Nav from "@/components/nav";
import Footer from "@/components/footer";
import Dashboard from "@/components/dashboard";
import { copyConfig, flagsConfig, marketingConfig, productsConfig, siteConfig } from "@/config";
import { Button } from "@/components/ui/button";

const SectionHeader = ({
  eyebrow,
  title,
  subtitle,
}: {
  eyebrow?: string;
  title: string;
  subtitle?: string;
}) => (
  <div className="space-y-3">
    {eyebrow ? (
      <div className="text-xs uppercase tracking-[0.3em] text-muted-foreground">
        {eyebrow}
      </div>
    ) : null}
    <h2 className="text-3xl font-brand tracking-tight md:text-4xl">{title}</h2>
    {subtitle ? (
      <p className="max-w-2xl text-sm text-muted-foreground md:text-base">
        {subtitle}
      </p>
    ) : null}
  </div>
);

export default function Home() {
  const token = useAuthToken();
  const isAuthenticated = Boolean(token);
  const testimonials = marketingConfig.testimonials;
  const [openFaqItems, setOpenFaqItems] = useState<string[]>([]);
  const faqContentRefs = useRef<Record<string, HTMLDivElement | null>>({});

  const toggleFaq = (question: string) => {
    setOpenFaqItems((prev) =>
      prev.includes(question)
        ? prev.filter((item) => item !== question)
        : [...prev, question]
    );
  };

  if (token === undefined) {
    return null;
  }

  return (
    <>
      <Nav />
      {isAuthenticated && flagsConfig.showDashboard ? (
        <Dashboard />
      ) : (
        <main>
          <section className="mx-auto w-full max-w-6xl border-x border-b border-border frame-corners-top frame-corners-bottom">
            <div className="grid lg:grid-cols-[1.1fr_0.9fr]">
            <div className="space-y-6 p-6 pb-12 pt-8 md:p-10 md:pb-14 lg:py-10 lg:pl-10 lg:pr-7">
              <div className="inline-flex items-center gap-2 rounded-full border border-border bg-background/70 px-3 py-1 text-xs uppercase tracking-[0.3em] text-muted-foreground">
                {marketingConfig.hero.eyebrow}
              </div>
              <h1 className="text-4xl font-display leading-tight md:text-6xl">
                {marketingConfig.hero.title}
              </h1>
              <p className="text-base text-muted-foreground md:text-lg">
                {marketingConfig.hero.subtitle}
              </p>
              <div className="flex flex-wrap gap-3">
                <Button asChild>
                  <Link href={marketingConfig.hero.primaryCta.href}>
                    {marketingConfig.hero.primaryCta.label}
                  </Link>
                </Button>
                <Button variant="outline" asChild>
                  <Link href={marketingConfig.hero.secondaryCta.href}>
                    {marketingConfig.hero.secondaryCta.label}
                  </Link>
                </Button>
              </div>
              <div className="grid gap-2 text-sm text-muted-foreground">
                {marketingConfig.hero.bullets.map((bullet) => (
                  <div key={bullet} className="flex items-center gap-2">
                    <span className="size-1.5 rounded-full bg-[var(--brand)]" />
                    {bullet}
                  </div>
                ))}
              </div>
            </div>
            <div className="border-t border-border p-6 md:p-10 lg:border-l lg:border-t-0 lg:py-10 lg:pl-7 lg:pr-10">
              <div className="flex h-full flex-col justify-evenly gap-6">
                <div className="mx-auto w-full max-w-sm text-left">
                  <div className="text-xs uppercase tracking-[0.3em] text-muted-foreground">
                    {siteConfig.brand.tagline}
                  </div>
                  <div className="mt-3 text-2xl font-brand">
                    {siteConfig.brand.description}
                  </div>
                </div>
                <div className="mx-auto grid w-full max-w-sm gap-4">
                  {marketingConfig.stats.map((stat) => (
                    <div
                      key={stat.label}
                      className="flex items-center justify-between"
                    >
                      <div>
                        <div className="text-xs text-muted-foreground">{stat.label}</div>
                        <div className="text-lg font-brand">{stat.value}</div>
                      </div>
                      <div className="ml-4 mr-2 text-right text-xs text-muted-foreground">{stat.detail}</div>
                    </div>
                  ))}
                </div>
              </div>
            </div>
            </div>
          </section>

          {flagsConfig.showProductSection ? (
          <section id="product" className="bg-noise-gradient relative isolate mx-auto w-full max-w-6xl border-x border-b border-border frame-corners-top frame-corners-bottom py-16">
            <div
              aria-hidden
              className="pointer-events-none absolute inset-0 z-0 bg-[url('/noise.svg')] bg-repeat opacity-[0.5] [background-size:160px_160px] dark:opacity-[0.28] dark:[background-size:180px_180px]"
            />
            <div className="relative z-10 px-6">
              <SectionHeader
                eyebrow={copyConfig.home.productsEyebrow}
                title={copyConfig.home.productsTitle}
                subtitle={copyConfig.home.productsSubtitle}
              />
              <div className="mt-10 grid gap-6 md:grid-cols-3">
                {productsConfig.products.map((product) => (
                  <div
                    key={product.id}
                    className="relative z-30 flex h-full flex-col justify-between rounded-none border border-border bg-background p-6"
                  >
                    <div className="space-y-3">
                      <span className="inline-flex rounded-full border border-border px-2 py-1 text-[0.65rem] uppercase tracking-[0.3em] text-muted-foreground">
                        {product.badge}
                      </span>
                      <h3 className="text-xl font-brand">{product.name}</h3>
                      <p className="text-sm text-muted-foreground">{product.tagline}</p>
                      <p className="text-sm">{product.description}</p>
                      <ul className="space-y-2 text-sm text-muted-foreground">
                        {product.bullets.map((bullet) => (
                          <li key={bullet} className="flex items-center gap-2">
                            <span className="size-1.5 rounded-full bg-[var(--brand)]" />
                            {bullet}
                          </li>
                        ))}
                      </ul>
                    </div>
                    <Button variant="outline" className="mt-6" asChild>
                      <Link href={product.cta.href}>{product.cta.label}</Link>
                    </Button>
                  </div>
                ))}
              </div>
            </div>
          </section>
          ) : null}

          {flagsConfig.showFeaturesSection ? (
          <section id="features" className="mx-auto w-full max-w-6xl border-x border-b border-border frame-corners-top frame-corners-bottom px-6 py-16">
            <SectionHeader
              eyebrow={copyConfig.home.featuresEyebrow}
              title="Everything is wired for you"
              subtitle="Auth, dashboard, and marketing templates are already in place. Update the JSON and ship."
            />
            <div className="mt-10 grid gap-6 md:grid-cols-2">
              {marketingConfig.features.map((feature) => (
                <div key={feature.title} className="rounded-none border border-border bg-background p-6">
                  <h3 className="text-lg font-brand">{feature.title}</h3>
                  <p className="mt-2 text-sm text-muted-foreground">{feature.description}</p>
                </div>
              ))}
            </div>
          </section>
          ) : null}

          <section className="mx-auto w-full max-w-6xl border-x border-b border-border frame-corners-top frame-corners-bottom px-6 py-16">
            <div className="grid gap-8 lg:grid-cols-2">
              {marketingConfig.sections.map((section) => (
                <div key={section.id} className="rounded-none border border-border bg-background p-6">
                  <h3 className="text-xl font-brand">{section.title}</h3>
                  <p className="mt-3 text-sm text-muted-foreground">{section.body}</p>
                  <ul className="mt-4 space-y-2 text-sm text-muted-foreground">
                    {section.bullets.map((bullet) => (
                      <li key={bullet} className="flex items-center gap-2">
                        <span className="size-1.5 rounded-full bg-[var(--brand)]" />
                        {bullet}
                      </li>
                    ))}
                  </ul>
                </div>
              ))}
            </div>
          </section>

          {flagsConfig.showPricingSection ? (
          <section id="pricing" className="bg-noise-gradient relative isolate mx-auto w-full max-w-6xl border-x border-b border-border frame-corners-top frame-corners-bottom py-16">
            <div
              aria-hidden
              className="pointer-events-none absolute inset-0 z-0 bg-[url('/noise.svg')] bg-repeat opacity-[0.5] [background-size:160px_160px] dark:opacity-[0.28] dark:[background-size:180px_180px]"
            />
            <div className="relative z-10 px-6">
              <SectionHeader
                eyebrow={productsConfig.pricing.eyebrow}
                title={productsConfig.pricing.title}
                subtitle={productsConfig.pricing.subtitle}
              />
              <div className="mt-10 grid gap-6 md:grid-cols-3">
                {productsConfig.pricing.plans.map((plan) => (
                  <div
                    key={plan.name}
                    className={`flex h-full flex-col justify-between rounded-none border p-6 ${
                      plan.highlighted
                        ? "border-[var(--brand)] bg-background"
                        : "border-border bg-background"
                    }`}
                  >
                    <div className="space-y-3">
                      <h3 className="text-lg font-brand">{plan.name}</h3>
                      <div className="text-3xl font-brand">
                        {plan.price}
                        <span className="text-xs text-muted-foreground"> {plan.cadence}</span>
                      </div>
                      <p className="text-sm text-muted-foreground">{plan.description}</p>
                      <ul className="space-y-2 text-sm text-muted-foreground">
                        {plan.features.map((feature) => (
                          <li key={feature} className="flex items-center gap-2">
                            <span className="size-1.5 rounded-full bg-[var(--brand)]" />
                            {feature}
                          </li>
                        ))}
                      </ul>
                    </div>
                    <Button className="mt-6" variant={plan.highlighted ? "default" : "outline"} asChild>
                      <Link href={plan.cta.href}>{plan.cta.label}</Link>
                    </Button>
                  </div>
                ))}
              </div>
              <p className="mt-6 text-xs text-muted-foreground">
                {productsConfig.pricing.disclaimer}
              </p>
            </div>
          </section>
          ) : null}

          {flagsConfig.showTestimonialsSection ? (
          <section className="mx-auto w-full max-w-6xl border-x border-b border-border frame-corners-top frame-corners-bottom py-16">
            <div className="px-6">
              <SectionHeader
                eyebrow={copyConfig.home.testimonialsEyebrow}
                title="Teams keep shipping with config"
                subtitle="A few notes from teams that replaced hard-coded marketing sites."
              />
            </div>
            <div className="mt-10 overflow-hidden">
              <div className="testimonial-track flex w-max gap-6">
                {[...testimonials, ...testimonials].map((testimonial, index) => (
                  <div
                    key={`${testimonial.name}-${index}`}
                    className="w-[16.5rem] shrink-0 rounded-lg border border-border bg-background px-5 py-7 md:w-[18.5rem] md:px-6 md:py-8"
                  >
                    <div className="flex items-center gap-3">
                      {testimonial.avatar ? (
                        <Image
                          src={testimonial.avatar}
                          alt={`${testimonial.name} avatar`}
                          width={44}
                          height={44}
                          className="size-11 rounded-full object-cover"
                        />
                      ) : (
                        <div className="flex size-11 items-center justify-center rounded-full border border-border text-xs font-semibold text-muted-foreground">
                          {testimonial.name
                            .split(" ")
                            .map((part) => part[0])
                            .join("")
                            .slice(0, 2)
                            .toUpperCase()}
                        </div>
                      )}
                      <div className="text-xs text-muted-foreground">
                        <div className="font-medium text-foreground">{testimonial.name}</div>
                        <div>
                          {testimonial.role}, {testimonial.company}
                        </div>
                      </div>
                    </div>
                    <p className="mt-5 text-sm leading-relaxed">“{testimonial.quote}”</p>
                  </div>
                ))}
              </div>
            </div>
          </section>
          ) : null}

          {flagsConfig.showFaqSection ? (
          <section id="faq" className="mx-auto w-full max-w-6xl border-x border-b border-border frame-corners-top frame-corners-bottom py-16">
            <div className="px-6">
              <SectionHeader
                eyebrow="FAQ"
                title="Questions, answered"
                subtitle="Everything you need to hand off to your team."
              />
            </div>
            <div className="mt-8 w-full divide-y divide-border border-y border-border">
              {marketingConfig.faq.map((item) => {
                const isOpen = openFaqItems.includes(item.question);

                return (
                  <div key={item.question} className="w-full bg-background px-6 py-4">
                    <button
                      type="button"
                      onClick={() => toggleFaq(item.question)}
                      className="flex w-full cursor-pointer items-center gap-2 text-left text-sm"
                    >
                      <span className="relative inline-flex size-3 shrink-0 items-center justify-center text-[0.55rem] leading-none text-muted-foreground">
                        <span
                          className={`absolute inset-0 flex items-center justify-center transition-opacity duration-200 ${
                            isOpen ? "opacity-0" : "opacity-100"
                          }`}
                        >
                          +
                        </span>
                        <span
                          className={`absolute inset-0 flex items-center justify-center transition-opacity duration-200 ${
                            isOpen ? "opacity-100" : "opacity-0"
                          }`}
                        >
                          -
                        </span>
                      </span>
                      <span
                        className={`font-medium transition-colors duration-200 ${
                          isOpen ? "text-foreground" : "text-muted-foreground/80"
                        }`}
                      >
                        {item.question}
                      </span>
                    </button>
                    <div
                      ref={(element) => {
                        faqContentRefs.current[item.question] = element;
                      }}
                      style={{
                        maxHeight: isOpen
                          ? `${faqContentRefs.current[item.question]?.scrollHeight ?? 0}px`
                          : "0px",
                      }}
                      className={`ml-5 overflow-hidden transition-[max-height,opacity,margin-top] duration-400 ease-[cubic-bezier(0.22,1,0.36,1)] ${
                        isOpen ? "mt-3 opacity-100" : "mt-0 opacity-0"
                      }`}
                    >
                      <p className="text-sm text-muted-foreground">{item.answer}</p>
                    </div>
                  </div>
                );
              })}
            </div>
          </section>
          ) : null}

          {flagsConfig.showFinalCta ? (
          <section className="mx-auto w-full max-w-6xl border-x border-b border-border frame-corners-top frame-corners-bottom bg-[var(--brand)]/10 px-6 py-16 md:px-8 md:py-20">
            <SectionHeader
              eyebrow={copyConfig.home.readyEyebrow}
              title="Make your product story editable"
              subtitle="Point your team to /config and start shipping updates in minutes."
            />
            <div className="mt-6 flex flex-wrap gap-3">
              <Button asChild>
                <Link href={marketingConfig.hero.primaryCta.href}>
                  {marketingConfig.hero.primaryCta.label}
                </Link>
              </Button>
            </div>
          </section>
          ) : null}
        </main>
      )}
      <Footer />
      <style jsx>{`
        .testimonial-track {
          animation: testimonial-scroll 48s linear infinite;
          will-change: transform;
        }

        @keyframes testimonial-scroll {
          from {
            transform: translateX(-50%);
          }
          to {
            transform: translateX(0%);
          }
        }
      `}</style>
    </>
  );
}
