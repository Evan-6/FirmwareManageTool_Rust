import type { ReactNode } from "react";

interface Props {
  title: string;
  actions?: ReactNode;
  children: ReactNode;
}

export function Panel({ title, actions, children }: Props) {
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>{title}</h2>
        <div className="panel-actions">{actions}</div>
      </header>
      <div className="panel-body">{children}</div>
    </section>
  );
}
