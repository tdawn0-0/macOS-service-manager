import type { Variants } from "motion/react";
import { motion, useAnimation } from "motion/react";

const variants: Variants = {
	normal: {
		opacity: 1,
		pathLength: 1,
		pathOffset: 0,
		transition: {
			duration: 0.4,
			opacity: { duration: 0.1 },
		},
	},
	animate: {
		opacity: [0, 1],
		pathLength: [0, 1],
		pathOffset: [1, 0],
		transition: {
			duration: 0.6,
			ease: "linear",
			opacity: { duration: 0.1 },
		},
	},
};

const ActivityIcon = ({
	size = 28,
	color,
}: { size?: number; color: string }) => {
	const controls = useAnimation();

	return (
		<div
			className="flex cursor-pointer select-none items-center justify-center rounded-md p-2 transition-colors duration-200 hover:bg-accent"
			onMouseEnter={() => {
				void controls.start("animate");
			}}
			onMouseLeave={() => {
				void controls.start("normal");
			}}
			style={{ color: color }}
		>
			<svg
				xmlns="http://www.w3.org/2000/svg"
				width={size}
				height={size}
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				strokeWidth="2"
				strokeLinecap="round"
				strokeLinejoin="round"
			>
				<title id="activity">Activity</title>
				<motion.path
					variants={variants}
					animate={controls}
					initial="normal"
					d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2"
				/>
			</svg>
		</div>
	);
};

export { ActivityIcon };
