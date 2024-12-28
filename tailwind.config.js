import { nextui } from "@nextui-org/react";
import flattenColorPalette from "tailwindcss/lib/util/flattenColorPalette";
import svgToDataUri from "mini-svg-data-uri";

/** @type {import('tailwindcss').Config} */
export default {
	content: [
		"./src/**/*.{js,ts,jsx,tsx}",
		"./node_modules/@nextui-org/theme/dist/**/*.{js,ts,jsx,tsx}",
	],
	theme: {
		extend: {},
	},
	darkMode: "class",
	plugins: [nextui(), addVariablesForColors, dotBackground],
};

function addVariablesForColors({ addBase, theme }) {
	let allColors = flattenColorPalette(theme("colors"));
	let newVars = Object.fromEntries(
		Object.entries(allColors).map(([key, val]) => [`--${key}`, val]),
	);

	addBase({
		":root": newVars,
	});
}

function dotBackground({ matchUtilities, theme }) {
	matchUtilities(
		{
			"bg-grid": (value) => ({
				backgroundImage: `url("${svgToDataUri(
					`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" width="32" height="32" fill="none" stroke="${value}"><path d="M0 .5H31.5V32"/></svg>`,
				)}")`,
			}),
			"bg-grid-small": (value) => ({
				backgroundImage: `url("${svgToDataUri(
					`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" width="8" height="8" fill="none" stroke="${value}"><path d="M0 .5H31.5V32"/></svg>`,
				)}")`,
			}),
			"bg-dot": (value) => ({
				backgroundImage: `url("${svgToDataUri(
					`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" width="16" height="16" fill="none"><circle fill="${value}" id="pattern-circle" cx="10" cy="10" r="1.6257413380501518"></circle></svg>`,
				)}")`,
			}),
		},
		{ values: flattenColorPalette(theme("backgroundColor")), type: "color" },
	);
}
