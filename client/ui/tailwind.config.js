/** @type {import('tailwindcss').Config} */
module.exports = {
    darkMode: ['class'],
    content: [
    './src/pages/**/*.{js,ts,jsx,tsx,mdx}',
    './src/components/**/*.{js,ts,jsx,tsx,mdx}',
    './src/app/**/*.{js,ts,jsx,tsx,mdx}',
  ],
  theme: {
  	extend: {
  		fontFamily: {
  			// Google Sans everywhere (self-hosted, see src/styles/google-sans.css).
  			sans: [
  				'"Google Sans"',
  				'"Segoe UI"',
  				'system-ui',
  				'sans-serif'
  			]
  		},
  		colors: {
  			// Themeable Tailwind scales: light values are Tailwind's defaults, dark values are
  			// Midnight Plum (src/app/theme.css, generated).
  			gray: {
  				'50': 'rgb(var(--gray-50) / <alpha-value>)',
  				'100': 'rgb(var(--gray-100) / <alpha-value>)',
  				'200': 'rgb(var(--gray-200) / <alpha-value>)',
  				'300': 'rgb(var(--gray-300) / <alpha-value>)',
  				'400': 'rgb(var(--gray-400) / <alpha-value>)',
  				'500': 'rgb(var(--gray-500) / <alpha-value>)',
  				'600': 'rgb(var(--gray-600) / <alpha-value>)',
  				'700': 'rgb(var(--gray-700) / <alpha-value>)',
  				'800': 'rgb(var(--gray-800) / <alpha-value>)',
  				'900': 'rgb(var(--gray-900) / <alpha-value>)',
  				'950': 'rgb(var(--gray-950) / <alpha-value>)',
  			},
  			blue: {
  				'50': 'rgb(var(--blue-50) / <alpha-value>)',
  				'100': 'rgb(var(--blue-100) / <alpha-value>)',
  				'200': 'rgb(var(--blue-200) / <alpha-value>)',
  				'300': 'rgb(var(--blue-300) / <alpha-value>)',
  				'400': 'rgb(var(--blue-400) / <alpha-value>)',
  				'500': 'rgb(var(--blue-500) / <alpha-value>)',
  				'600': 'rgb(var(--blue-600) / <alpha-value>)',
  				'700': 'rgb(var(--blue-700) / <alpha-value>)',
  				'800': 'rgb(var(--blue-800) / <alpha-value>)',
  				'900': 'rgb(var(--blue-900) / <alpha-value>)',
  				'950': 'rgb(var(--blue-950) / <alpha-value>)',
  			},
  			red: {
  				'50': 'rgb(var(--red-50) / <alpha-value>)',
  				'100': 'rgb(var(--red-100) / <alpha-value>)',
  				'200': 'rgb(var(--red-200) / <alpha-value>)',
  				'300': 'rgb(var(--red-300) / <alpha-value>)',
  				'400': 'rgb(var(--red-400) / <alpha-value>)',
  				'500': 'rgb(var(--red-500) / <alpha-value>)',
  				'600': 'rgb(var(--red-600) / <alpha-value>)',
  				'700': 'rgb(var(--red-700) / <alpha-value>)',
  				'800': 'rgb(var(--red-800) / <alpha-value>)',
  				'900': 'rgb(var(--red-900) / <alpha-value>)',
  				'950': 'rgb(var(--red-950) / <alpha-value>)',
  			},
  			green: {
  				'50': 'rgb(var(--green-50) / <alpha-value>)',
  				'100': 'rgb(var(--green-100) / <alpha-value>)',
  				'200': 'rgb(var(--green-200) / <alpha-value>)',
  				'300': 'rgb(var(--green-300) / <alpha-value>)',
  				'400': 'rgb(var(--green-400) / <alpha-value>)',
  				'500': 'rgb(var(--green-500) / <alpha-value>)',
  				'600': 'rgb(var(--green-600) / <alpha-value>)',
  				'700': 'rgb(var(--green-700) / <alpha-value>)',
  				'800': 'rgb(var(--green-800) / <alpha-value>)',
  				'900': 'rgb(var(--green-900) / <alpha-value>)',
  				'950': 'rgb(var(--green-950) / <alpha-value>)',
  			},
  			amber: {
  				'50': 'rgb(var(--amber-50) / <alpha-value>)',
  				'100': 'rgb(var(--amber-100) / <alpha-value>)',
  				'200': 'rgb(var(--amber-200) / <alpha-value>)',
  				'300': 'rgb(var(--amber-300) / <alpha-value>)',
  				'400': 'rgb(var(--amber-400) / <alpha-value>)',
  				'500': 'rgb(var(--amber-500) / <alpha-value>)',
  				'600': 'rgb(var(--amber-600) / <alpha-value>)',
  				'700': 'rgb(var(--amber-700) / <alpha-value>)',
  				'800': 'rgb(var(--amber-800) / <alpha-value>)',
  				'900': 'rgb(var(--amber-900) / <alpha-value>)',
  				'950': 'rgb(var(--amber-950) / <alpha-value>)',
  			},
  			orange: {
  				'50': 'rgb(var(--orange-50) / <alpha-value>)',
  				'100': 'rgb(var(--orange-100) / <alpha-value>)',
  				'200': 'rgb(var(--orange-200) / <alpha-value>)',
  				'300': 'rgb(var(--orange-300) / <alpha-value>)',
  				'400': 'rgb(var(--orange-400) / <alpha-value>)',
  				'500': 'rgb(var(--orange-500) / <alpha-value>)',
  				'600': 'rgb(var(--orange-600) / <alpha-value>)',
  				'700': 'rgb(var(--orange-700) / <alpha-value>)',
  				'800': 'rgb(var(--orange-800) / <alpha-value>)',
  				'900': 'rgb(var(--orange-900) / <alpha-value>)',
  				'950': 'rgb(var(--orange-950) / <alpha-value>)',
  			},
  			yellow: {
  				'50': 'rgb(var(--yellow-50) / <alpha-value>)',
  				'100': 'rgb(var(--yellow-100) / <alpha-value>)',
  				'200': 'rgb(var(--yellow-200) / <alpha-value>)',
  				'300': 'rgb(var(--yellow-300) / <alpha-value>)',
  				'400': 'rgb(var(--yellow-400) / <alpha-value>)',
  				'500': 'rgb(var(--yellow-500) / <alpha-value>)',
  				'600': 'rgb(var(--yellow-600) / <alpha-value>)',
  				'700': 'rgb(var(--yellow-700) / <alpha-value>)',
  				'800': 'rgb(var(--yellow-800) / <alpha-value>)',
  				'900': 'rgb(var(--yellow-900) / <alpha-value>)',
  				'950': 'rgb(var(--yellow-950) / <alpha-value>)',
  			},
  			background: 'hsl(var(--background))',
  			foreground: 'hsl(var(--foreground))',
  			border: 'hsl(var(--border))',
  			input: 'hsl(var(--input))',
  			ring: 'hsl(var(--ring))',
  			primary: {
  				DEFAULT: 'hsl(var(--primary))',
  				foreground: 'hsl(var(--primary-foreground))'
  			},
  			secondary: {
  				DEFAULT: 'hsl(var(--secondary))',
  				foreground: 'hsl(var(--secondary-foreground))'
  			},
  			tertiary: '#64748b',
  			card: {
  				DEFAULT: 'hsl(var(--card))',
  				foreground: 'hsl(var(--card-foreground))'
  			},
  			popover: {
  				DEFAULT: 'hsl(var(--popover))',
  				foreground: 'hsl(var(--popover-foreground))'
  			},
  			muted: {
  				DEFAULT: 'hsl(var(--muted))',
  				foreground: 'hsl(var(--muted-foreground))'
  			},
  			accent: {
  				DEFAULT: 'hsl(var(--accent))',
  				foreground: 'hsl(var(--accent-foreground))'
  			},
  			destructive: {
  				DEFAULT: 'hsl(var(--destructive))',
  				foreground: 'hsl(var(--destructive-foreground))'
  			},
  			chart: {
  				'1': 'hsl(var(--chart-1))',
  				'2': 'hsl(var(--chart-2))',
  				'3': 'hsl(var(--chart-3))',
  				'4': 'hsl(var(--chart-4))',
  				'5': 'hsl(var(--chart-5))'
  			},
  			sidebar: {
  				DEFAULT: 'hsl(var(--sidebar-background))',
  				foreground: 'hsl(var(--sidebar-foreground))',
  				primary: 'hsl(var(--sidebar-primary))',
  				'primary-foreground': 'hsl(var(--sidebar-primary-foreground))',
  				accent: 'hsl(var(--sidebar-accent))',
  				'accent-foreground': 'hsl(var(--sidebar-accent-foreground))',
  				border: 'hsl(var(--sidebar-border))',
  				ring: 'hsl(var(--sidebar-ring))'
  			}
  		},
  		borderRadius: {
  			lg: 'var(--radius)',
  			md: 'calc(var(--radius) - 2px)',
  			sm: 'calc(var(--radius) - 4px)'
  		},
  		keyframes: {
  			'accordion-down': {
  				from: {
  					height: '0'
  				},
  				to: {
  					height: 'var(--radix-accordion-content-height)'
  				}
  			},
  			'accordion-up': {
  				from: {
  					height: 'var(--radix-accordion-content-height)'
  				},
  				to: {
  					height: '0'
  				}
  			},
  			// From Anarlog (MIT, see ANARLOG-LICENSE.md): the overlay's audio sticks.
  			'anarlog-dancing-stick': {
  				'0%, 100%': {
  					transform: 'scaleY(0.2)'
  				},
  				'50%': {
  					transform: 'scaleY(1)'
  				}
  			}
  		},
  		animation: {
  			'accordion-down': 'accordion-down 0.2s ease-out',
  			'anarlog-dancing-stick': 'anarlog-dancing-stick 1s ease-in-out infinite',
  			'accordion-up': 'accordion-up 0.2s ease-out'
  		}
  	}
  },
  plugins: [
    require("tailwindcss-animate"),
    require("@tailwindcss/typography"),
    require("@tailwindcss/container-queries"),
  ],
}