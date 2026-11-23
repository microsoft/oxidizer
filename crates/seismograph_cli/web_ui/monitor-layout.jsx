// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import { createContext, useContext, useEffect, useRef, useState } from "react";
import { Orientation, SplitviewReact } from "dockview-react";

const Contents = createContext(null);
const narrowScreen = "(max-width: 650px)";

function SplitContent({ api }) {
  return useContext(Contents)[api.id];
}

const components = { content: SplitContent };

export function MonitorLayout({ sidebar, children }) {
  const [vertical, setVertical] = useState(() => matchMedia(narrowScreen).matches);
  const [api, setApi] = useState(null);
  const [warning, setWarning] = useState("");
  const container = useRef(null);

  useEffect(() => {
    const media = matchMedia(narrowScreen);
    const update = () => setVertical(media.matches);
    media.addEventListener("change", update);
    update();
    return () => media.removeEventListener("change", update);
  }, []);

  useEffect(() => {
    if (!api) return;
    const orientation = vertical ? Orientation.VERTICAL : Orientation.HORIZONTAL;
    const key = `seismograph.sidebar.v1:${orientation}`;
    const element = container.current;
    const minLimit = vertical ? 96 : 152;
    const maxLimit = vertical ? 320 : 440;
    const reportLimit = vertical ? 120 : 280;
    const initial = vertical ? 154 : 210;
    let saved = initial;
    try {
      const value = Number(localStorage.getItem(key));
      if (value > 0 && value <= maxLimit) saved = value;
    } catch (error) {
      setWarning(`Could not read the application pane size. ${error.message}`);
    }

    api.updateOptions({ orientation });
    const applications = api.getPanel("applications").api;
    const report = api.getPanel("report").api;
    const divider = element.querySelector(".monitor-split > .dv-split-view-container > .dv-sash-container > .dv-sash");
    const size = () => vertical ? applications.height : applications.width;
    let min;
    let max;
    function syncDivider() {
      divider?.setAttribute("aria-valuenow", String(Math.round(size())));
    }
    function fit() {
      const { clientWidth, clientHeight } = element;
      const available = vertical ? clientHeight : clientWidth;
      const reportMin = Math.min(reportLimit, Math.floor(available * .55));
      max = Math.min(maxLimit, Math.max(0, available - reportMin));
      min = Math.min(minLimit, max, Math.floor(available * .35));
      report.setConstraints({ minimumSize: reportMin });
      applications.setConstraints({ minimumSize: min, maximumSize: max });
      api.layout(clientWidth, clientHeight);
      applications.setSize({ size: Math.max(min, Math.min(max, saved)) });
      divider?.setAttribute("aria-valuemin", String(min));
      divider?.setAttribute("aria-valuemax", String(max));
      syncDivider();
    }
    function saveSize() {
      saved = Math.round(size());
      syncDivider();
      try {
        localStorage.setItem(key, String(saved));
      } catch (error) {
        setWarning(`Could not save the application pane size. ${error.message}`);
      }
    }
    function resizeByKey(event) {
      const decrease = vertical ? "ArrowUp" : "ArrowLeft";
      const increase = vertical ? "ArrowDown" : "ArrowRight";
      if (![decrease, increase, "Home", "End"].includes(event.key)) return;
      event.preventDefault();
      const next = event.key === "Home" ? min : event.key === "End" ? max : size() + (event.key === increase ? 16 : -16);
      applications.setSize({ size: Math.max(min, Math.min(max, next)) });
      saveSize();
    }
    if (divider) {
      divider.tabIndex = 0;
      divider.setAttribute("role", "separator");
      divider.setAttribute("aria-label", "Resize applications");
      divider.setAttribute("aria-controls", "application-list");
      divider.setAttribute("aria-orientation", vertical ? "horizontal" : "vertical");
      divider.addEventListener("keydown", resizeByKey);
    }
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    const subscription = api.onDidLayoutChange(saveSize);
    return () => {
      observer.disconnect();
      subscription.dispose();
      divider?.removeEventListener("keydown", resizeByKey);
    };
  }, [api, vertical]);

  return <>
    {warning && <p className="error notice" role="status">{warning}</p>}
    <div className="layout" ref={container}>
      <Contents.Provider value={{ applications: sidebar, report: children }}>
        <SplitviewReact className="monitor-split" orientation={vertical ? Orientation.VERTICAL : Orientation.HORIZONTAL}
          disableAutoResizing proportionalLayout={false} components={components} onReady={({ api: split }) => {
            split.addPanel({ id: "applications", component: "content" });
            split.addPanel({ id: "report", component: "content" });
            setApi(split);
          }} />
      </Contents.Provider>
    </div>
  </>;
}
