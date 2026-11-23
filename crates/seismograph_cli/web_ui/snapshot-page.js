// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import { useEffect, useState } from "react";
import { api } from "./api.js";
import { readPage } from "./snapshot-state.js";

export function useSnapshotPage({ snapshot, section, parent = "", offset, fullStack, by, descending, active = true }) {
  const [result, setResult] = useState(null);
  const [attempt, setAttempt] = useState(0);
  const { id, viewId = "" } = snapshot;
  const scope = `${id}:${viewId}`;
  const key = JSON.stringify([scope, section, parent, offset, fullStack, by, descending, attempt]);
  useEffect(() => {
    if (!active) return;
    const controller = new AbortController();
    const { signal } = controller;
    async function read() {
      try {
        const page = await readPage(start => api.inspect(id, viewId, section, start, parent, fullStack, by, descending, signal), offset);
        if (!signal.aborted) setResult({ key, ...page });
      } catch (error) {
        if (!signal.aborted) setResult({ key, error });
      }
    }
    void read();
    return () => controller.abort();
  }, [active, key]);
  return {
    ...(result?.key === key ? result : {}),
    retry: () => setAttempt(value => value + 1),
  };
}
