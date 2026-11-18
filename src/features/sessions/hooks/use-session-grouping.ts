import { useState } from "react";

export function useSessionGrouping() {
  const [grouped, setGrouped] = useState(
    () => localStorage.getItem("local-agent.session-grouping") !== "time",
  );
  return {
    grouped,
    setGrouped: (value: boolean) => {
      setGrouped(value);
      localStorage.setItem(
        "local-agent.session-grouping",
        value ? "project" : "time",
      );
    },
  };
}
