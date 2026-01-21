import { useQuery } from "@tanstack/react-query";
import { commands } from "../ipc/tauri/bindings.ts";
import { LaunchdServiceListItem } from "../components/launchd-service-list-item.tsx";

export const LAUNCHD_LIST_QUERY_KEY = "launchdServiceList" as const;

export function LaunchdServiceListView() {
	const query = useQuery({
		queryKey: [LAUNCHD_LIST_QUERY_KEY],
		queryFn: async () => {
			const res = await commands.getLaunchdServices();
			if (res.status === "error") {
				throw new Error(res.error);
			}
			return res.data;
		},
	});

	if (query.isError) {
		return <div>Error: {query.error.message}</div>;
	}

	if (query.isLoading) {
		return <div>Loading...</div>;
	}

	if (!query.data || query.data.length === 0) {
		return (
			<div className="text-center text-default-500 py-8">
				No LaunchAgents found
			</div>
		);
	}

	return (
		<div className="flex flex-col flex-nowrap gap-2 flex-grow min-h-0">
			{query.data.map((launchdService) => (
				<LaunchdServiceListItem
					key={launchdService.path}
					launchdService={launchdService}
				/>
			))}
		</div>
	);
}
