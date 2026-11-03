import {
  Component,
  createContext,
  createElement,
  lazy,
  useContext,
  type ComponentProps,
  type ComponentType,
  type LazyExoticComponent,
  type ReactNode,
} from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";

const LoadAttempt = createContext<object>({});

/** A failed React.lazy object caches its rejection; a retry needs a new object. */
// Match React.lazy's component constraint while preserving the inferred props.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function retryableLazy<Page extends ComponentType<any>>(
  load: () => Promise<{ default: Page }>,
): ComponentType<ComponentProps<Page>> {
  // Initial suspension discards hook memoization. Keep the lazy object outside
  // that mount attempt; weak keys release resources when a boundary is gone.
  const resources = new WeakMap<object, LazyExoticComponent<Page>>();
  return function RetryablePage(props: ComponentProps<Page>) {
    const attempt = useContext(LoadAttempt);
    let resource = resources.get(attempt);
    if (!resource) {
      resource = lazy(load);
      resources.set(attempt, resource);
    }
    return createElement(resource, props);
  };
}

type Props = {
  children: ReactNode;
  resetKey: string;
  scope?: "page" | "application";
};
type State = { failed: boolean; resetKey: string; loadKey: object };

export class RenderBoundary extends Component<Props, State> {
  state: State = {
    failed: false,
    resetKey: this.props.resetKey,
    loadKey: {},
  };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  static getDerivedStateFromProps(props: Props, state: State) {
    if (props.resetKey === state.resetKey) return null;
    return {
      resetKey: props.resetKey,
      failed: false,
      loadKey: state.failed ? {} : state.loadKey,
    };
  }

  private retry = () => {
    this.setState({ failed: false, loadKey: {} });
  };

  render() {
    if (!this.state.failed)
      return (
        <LoadAttempt.Provider value={this.state.loadKey}>
          {this.props.children}
        </LoadAttempt.Provider>
      );
    const application = this.props.scope === "application";
    return (
      <section
        aria-label={application ? "应用恢复" : "页面恢复"}
        className={
          application
            ? "flex h-dvh items-center justify-center bg-surface-base p-6"
            : "min-h-0 overflow-auto p-4 sm:p-6"
        }
      >
        <div className="max-w-xl space-y-4">
          <Alert status="danger">
            <AlertTitle>
              {application ? "界面暂时无法显示" : "此页面暂时无法显示"}
            </AlertTitle>
            <AlertDescription>
              {application
                ? "请重新打开界面。未保存的输入可能需要重新填写。"
                : "可以重试或使用导航打开其他页面。此页面未保存的输入可能需要重新填写。"}
            </AlertDescription>
          </Alert>
          <div className="flex flex-wrap gap-2">
            <Button variant="tertiary" onClick={this.retry}>
              {application ? "重新打开界面" : "重试页面"}
            </Button>
            {!application && (
              <Button
                variant="tertiary"
                onClick={() => {
                  this.retry();
                  window.location.hash = "/overview";
                }}
              >
                返回总览
              </Button>
            )}
            <Button variant="tertiary" onClick={() => window.location.reload()}>
              重新加载应用
            </Button>
          </div>
        </div>
      </section>
    );
  }
}

// Error messages/stacks may contain session text or credentials. Root logging
// uses a fixed category instead of serializing the thrown value or props.
export const renderErrorOptions = {
  onCaughtError: () => console.error("[local-agent] caught-render-error"),
  onUncaughtError: () => console.error("[local-agent] uncaught-render-error"),
};
