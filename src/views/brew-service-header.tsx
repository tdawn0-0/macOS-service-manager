import { Button } from "@nextui-org/react";
import { useQueryClient } from "@tanstack/react-query";
import { RotateCw } from "lucide-react";
import { BREW_LIST_QUERY_KEY } from "./brew-service-list.tsx";

export function BrewServiceHeader() {
	const queryClient = useQueryClient();

	const handleRefetch = () => {
		void queryClient.invalidateQueries({
			queryKey: [BREW_LIST_QUERY_KEY],
		});
	};

	return (
		<div className="mt-1 mb-4 flex flex-nowrap items-center justify-between">
			<p className="relative bg-gradient-to-b from-neutral-200 to-neutral-500 bg-clip-text font-bold text-2xl text-transparent">
				Homebrew Services
			</p>
			<div>
				<Button
					isIconOnly
					aria-label="refresh"
					size="sm"
					variant="light"
					onPress={handleRefetch}
				>
					<RotateCw color="#ccc" />
				</Button>
			</div>
		</div>
	);
}
