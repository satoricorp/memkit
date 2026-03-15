export type NavItem = {
  label: string;
  href: string;
};

export type Cta = {
  label: string;
  href: string;
};

export type SocialLink = {
  label: string;
  href: string;
};

export type SiteConfig = {
  brand: {
    name: string;
    tagline: string;
    description: string;
    logo: {
      text: string;
      accent: string;
    };
  };
  meta: {
    title: string;
    description: string;
  };
  nav: {
    items: NavItem[];
    cta: Cta;
  };
  footer: {
    headline: string;
    subhead: string;
    links: NavItem[];
    legal: string;
  };
  social: SocialLink[];
};

export type MarketingSection = {
  id: string;
  title: string;
  body: string;
  bullets: string[];
};

export type MarketingConfig = {
  hero: {
    eyebrow: string;
    title: string;
    subtitle: string;
    primaryCta: Cta;
    secondaryCta: Cta;
    bullets: string[];
  };
  stats: {
    label: string;
    value: string;
    detail: string;
  }[];
  sections: MarketingSection[];
  features: {
    title: string;
    description: string;
  }[];
  testimonials: {
    quote: string;
    name: string;
    role: string;
    company: string;
    avatar?: string;
  }[];
  faq: {
    question: string;
    answer: string;
  }[];
};

export type ProductConfig = {
  id: string;
  name: string;
  tagline: string;
  description: string;
  badge: string;
  bullets: string[];
  cta: Cta;
};

export type PricingPlan = {
  name: string;
  price: string;
  cadence: string;
  description: string;
  highlighted: boolean;
  features: string[];
  cta: Cta;
};

export type ProductsConfig = {
  products: ProductConfig[];
  pricing: {
    eyebrow: string;
    title: string;
    subtitle: string;
    plans: PricingPlan[];
    disclaimer: string;
  };
};

export type DashboardConfig = {
  welcome: {
    title: string;
    subtitle: string;
  };
  metrics: {
    label: string;
    value: string;
    change: string;
  }[];
  modules: {
    title: string;
    description: string;
    links: NavItem[];
  }[];
  activity: {
    title: string;
    description: string;
    time: string;
    status: "done" | "in-progress" | "queued";
  }[];
  quickActions: NavItem[];
};

export type AuthProviderId = "google" | "github";

export type AuthConfig = {
  redirectTo: string;
  signIn: {
    title: string;
    subtitle: string;
    footer: string;
  };
  signUp: {
    title: string;
    subtitle: string;
    footer: string;
  };
  providers: {
    id: AuthProviderId;
    label: string;
  }[];
};

export type FlagsConfig = {
  showProductSection: boolean;
  showFeaturesSection: boolean;
  showPricingSection: boolean;
  showTestimonialsSection: boolean;
  showFaqSection: boolean;
  showFinalCta: boolean;
  showDashboard: boolean;
};

export type CopyConfig = {
  home: {
    productsEyebrow: string;
    productsTitle: string;
    productsSubtitle: string;
    featuresEyebrow: string;
    testimonialsEyebrow: string;
    readyEyebrow: string;
  };
  dashboard: {
    recentActivityTitle: string;
    quickActionsTitle: string;
  };
};
